//! DocumentManager の世代と生存状態の台帳（Issue #50）。
//!
//! TSF は DocumentManager（DM）を COM ポインタで通知する。ブラウザなどはタブ切替の
//! たびに DM を破棄・再作成し、OS は破棄された DM のアドレスを次の DM に再利用する。
//! ポインタだけを鍵にすると、破棄後に遅延処理される古いフォーカス通知が死んだ DM の
//! 項目を `ModeStore` に入れ直し、同じアドレスの新しい DM がそれを引いてしまう。
//!
//! そのため DM を **ポインタ + 世代**（[`DmRef`]）で識別する。世代の登録・失効は
//! TSF の STA スレッド上で同期に届く `OnInitDocumentMgr` / `OnUninitDocumentMgr` /
//! `OnSetFocus` / Activate / Deactivate で行い、台帳は thread-local に置く
//! （ロック不要、欠落なし）。遅延処理は、キューに積んだ時点で確定した [`DmRef`] が
//! 今も生きているかを [`is_live`] で見る。処理時に最新世代を取り直して古いイベントに
//! 付け替えることはしない。
//!
//! 登録の経路は 2 つだけ: `OnInitDocumentMgr`（新しい DM）と Activate の
//! `EnumDocumentMgrs`（Activate 前から存在する DM）。`OnSetFocus` が台帳に無い ptr を
//! 見ても登録しない（**未知の ptr は死んだ扱い**）。初回の目撃で登録すると、下の掃除で
//! 忘れた死んだ DM への古い通知を生存中と誤認するため。
//!
//! 死んだ slot は、同じアドレスの新しい登録で上書きされるまで残す（以後に届く古い
//! イベントを「死んだ世代」と判定するため）。上限 [`DmRegistry::DEAD_SLOT_CAP`] を
//! 超えたら古い死んだ slot から捨てる。捨てた ptr への古い通知は「未知の ptr」に落ちる
//! ので、やはり生存とは判定されない。
//!
//! Deactivate では通知 sink が外れるので、その間に破棄された DM の Uninit は届かない。
//! Deactivate で生存 slot を全部失効させ、次の Activate の列挙で登録し直す。`ModeStore`
//! は Activate をまたいで残るので、世代番号は初期値に戻さず進め続ける。

use std::cell::RefCell;
use std::collections::HashMap;

/// DocumentManager の識別子。ポインタが再利用されても世代が違えば別物として扱う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmRef {
    /// `ITfDocumentMgr` の生ポインタ値。0 にはならない。
    pub ptr: usize,
    /// 登録ごとに単調増加する世代。
    pub generation: u64,
}

impl std::fmt::LowerHex for DmRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#x}/g{}", self.ptr, self.generation)
    }
}

#[derive(Clone, Copy, Debug)]
struct DmSlot {
    generation: u64,
    alive: bool,
}

/// 台帳本体。純粋なデータ構造で、単体テストはこれに対して行う。
#[derive(Debug, Default)]
pub struct DmRegistry {
    slots: HashMap<usize, DmSlot>,
    next_gen: u64,
    dead: usize,
}

impl DmRegistry {
    /// 残しておく死んだ slot の上限。超えたら世代の古いものから捨てる。
    pub const DEAD_SLOT_CAP: usize = 1024;

    fn alloc_gen(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    fn put(&mut self, ptr: usize, slot: DmSlot) {
        if let Some(old) = self.slots.insert(ptr, slot)
            && !old.alive
        {
            self.dead -= 1;
        }
        if !slot.alive {
            self.dead += 1;
            self.evict_dead_over_cap();
        }
    }

    /// `OnInitDocumentMgr`: 新しい世代として登録する。同じポインタの古い slot
    /// （生死を問わず）は上書きされる＝アドレス再利用の検出。
    pub fn register(&mut self, ptr: usize) -> DmRef {
        debug_assert!(ptr != 0);
        let generation = self.alloc_gen();
        self.put(
            ptr,
            DmSlot {
                generation,
                alive: true,
            },
        );
        DmRef { ptr, generation }
    }

    /// Activate の列挙 / `GetFocus()` の照合: 今生きていると分かった ptr を登録する。
    ///
    /// 生存中の slot があればその世代をそのまま返す（世代を進めない。sink 登録後に
    /// `OnInitDocumentMgr` で登録済みの DM が列挙にも現れたとき、先の [`DmRef`] を
    /// 無効にしないため）。slot が無いか死んでいれば新しい世代で登録する。
    /// 戻り値の `bool` は「新しく登録した」。
    pub fn ensure_live(&mut self, ptr: usize) -> (DmRef, bool) {
        debug_assert!(ptr != 0);
        match self.slots.get(&ptr) {
            Some(slot) if slot.alive => (
                DmRef {
                    ptr,
                    generation: slot.generation,
                },
                false,
            ),
            _ => (self.register(ptr), true),
        }
    }

    /// `OnSetFocus`: 目撃したポインタの [`DmRef`] を返す。slot があれば生死を問わず
    /// その世代（呼び出し側が [`is_live`](Self::is_live) で判定する）。台帳に無い ptr
    /// は `None`（登録しない。未知の ptr は死んだ扱い）。
    pub fn lookup(&self, ptr: usize) -> Option<DmRef> {
        self.slots.get(&ptr).map(|s| DmRef {
            ptr,
            generation: s.generation,
        })
    }

    /// `OnUninitDocumentMgr`: slot を失効させ、その [`DmRef`] を返す。
    ///
    /// 台帳に無い ptr は、新しい世代を割り当ててそのまま死んだ slot として記録する
    /// （以後その ptr に届く通知が死んだ世代に解決されるように）。戻り値の `bool` は
    /// 「生存中の slot を失効させた」（未知の ptr / 既に死んでいた ptr では `false`）。
    pub fn expire(&mut self, ptr: usize) -> (DmRef, bool) {
        debug_assert!(ptr != 0);
        match self.slots.get_mut(&ptr) {
            Some(slot) if slot.alive => {
                slot.alive = false;
                let dm = DmRef {
                    ptr,
                    generation: slot.generation,
                };
                self.dead += 1;
                self.evict_dead_over_cap();
                (dm, true)
            }
            Some(slot) => (
                DmRef {
                    ptr,
                    generation: slot.generation,
                },
                false,
            ),
            None => {
                let generation = self.alloc_gen();
                self.put(
                    ptr,
                    DmSlot {
                        generation,
                        alive: false,
                    },
                );
                (DmRef { ptr, generation }, false)
            }
        }
    }

    /// Deactivate: 生存中の slot を全部失効させる。戻り値は失効させた [`DmRef`]。
    pub fn expire_all(&mut self) -> Vec<DmRef> {
        let mut expired = Vec::new();
        for (&ptr, slot) in self.slots.iter_mut() {
            if slot.alive {
                slot.alive = false;
                expired.push(DmRef {
                    ptr,
                    generation: slot.generation,
                });
            }
        }
        self.dead += expired.len();
        self.evict_dead_over_cap();
        expired
    }

    /// slot があり、世代が一致し、生存しているときだけ `true`。
    pub fn is_live(&self, dm: DmRef) -> bool {
        self.slots
            .get(&dm.ptr)
            .is_some_and(|s| s.alive && s.generation == dm.generation)
    }

    /// 死んだ slot の数（テスト用）。
    #[cfg(test)]
    pub fn dead_count(&self) -> usize {
        self.dead
    }

    /// 生存中の slot の数（テスト用）。
    #[cfg(test)]
    pub fn live_count(&self) -> usize {
        self.slots.len() - self.dead
    }

    fn evict_dead_over_cap(&mut self) {
        while self.dead > Self::DEAD_SLOT_CAP {
            let oldest = self
                .slots
                .iter()
                .filter(|(_, s)| !s.alive)
                .min_by_key(|(_, s)| s.generation)
                .map(|(&ptr, _)| ptr);
            match oldest {
                Some(ptr) => {
                    self.slots.remove(&ptr);
                    self.dead -= 1;
                }
                None => break,
            }
        }
    }
}

/// `OnSetFocus` が目撃した DM ポインタの解決結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmSeen {
    /// DM なし（ポインタ 0）。
    Absent,
    /// 台帳に無い ptr。登録せず、死んだ扱い（Issue #50）。
    Unknown(usize),
    /// 台帳にある ptr（生死は [`is_live`] で判定する）。
    Known(DmRef),
}

impl DmSeen {
    /// 台帳にある DM なら [`DmRef`]。
    pub fn known(self) -> Option<DmRef> {
        match self {
            DmSeen::Known(dm) => Some(dm),
            _ => None,
        }
    }
}

/// `OnSetFocus` から。ptr を台帳で解決する（登録はしない）。
pub fn seen(ptr: usize) -> DmSeen {
    if ptr == 0 {
        return DmSeen::Absent;
    }
    match lookup(ptr) {
        Some(dm) => DmSeen::Known(dm),
        None => DmSeen::Unknown(ptr),
    }
}

thread_local! {
    /// TSF スレッドの台帳。TSF の通知はすべて同じ STA スレッドで届く。
    static TL_DM_REGISTRY: RefCell<DmRegistry> = RefCell::new(DmRegistry::default());
}

/// `OnInitDocumentMgr` から。
pub fn register(ptr: usize) -> DmRef {
    TL_DM_REGISTRY.with(|r| r.borrow_mut().register(ptr))
}

/// Activate の列挙 / `GetFocus()` の照合から。`(DmRef, 新しく登録したか)`。
pub fn ensure_live(ptr: usize) -> (DmRef, bool) {
    TL_DM_REGISTRY.with(|r| r.borrow_mut().ensure_live(ptr))
}

/// `OnSetFocus` から。`ptr == 0`（DM なし）と台帳に無い ptr は `None`。
pub fn lookup(ptr: usize) -> Option<DmRef> {
    if ptr == 0 {
        return None;
    }
    TL_DM_REGISTRY.with(|r| r.borrow().lookup(ptr))
}

/// `OnUninitDocumentMgr` から。失効させた（または死んだ slot として記録した）
/// [`DmRef`] と、生存中の slot を失効させたかを返す。
pub fn expire(ptr: usize) -> (DmRef, bool) {
    TL_DM_REGISTRY.with(|r| r.borrow_mut().expire(ptr))
}

/// Deactivate から。失効させた [`DmRef`] を返す。
pub fn expire_all() -> Vec<DmRef> {
    TL_DM_REGISTRY.with(|r| r.borrow_mut().expire_all())
}

/// 遅延処理・即時保存から。TSF スレッド以外では台帳が空なので常に `false`。
pub fn is_live(dm: DmRef) -> bool {
    TL_DM_REGISTRY
        .try_with(|r| r.borrow().is_live(dm))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_expire_marks_generation_dead() {
        let mut reg = DmRegistry::default();
        let a = reg.register(0x10);
        assert!(reg.is_live(a));
        assert_eq!(reg.expire(0x10), (a, true));
        assert!(!reg.is_live(a));
        // 二重の失効は同じ世代を返し、失効はしない
        assert_eq!(reg.expire(0x10), (a, false));
        assert_eq!(reg.dead_count(), 1);
    }

    /// 破棄後に同じアドレスで再初期化されると世代が進み、旧世代は生存と判定されない。
    #[test]
    fn same_address_reinit_gets_new_generation() {
        let mut reg = DmRegistry::default();
        let old = reg.register(0x10);
        reg.expire(0x10);
        let new = reg.register(0x10);
        assert_ne!(old, new);
        assert!(reg.is_live(new));
        assert!(!reg.is_live(old));
        // 上書きで死んだ slot は消えている
        assert_eq!(reg.dead_count(), 0);
        // 目撃は現在の世代を返す
        assert_eq!(reg.lookup(0x10), Some(new));
    }

    /// 台帳に無い ptr は目撃しても登録されない（未知の ptr は死んだ扱い）。
    #[test]
    fn lookup_does_not_register_unknown_pointer() {
        let mut reg = DmRegistry::default();
        assert_eq!(reg.lookup(0x20), None);
        assert_eq!(reg.live_count(), 0);
        // 登録してから目撃すれば世代が返る
        let a = reg.register(0x20);
        assert_eq!(reg.lookup(0x20), Some(a));
    }

    /// 破棄後に届いた通知は死んだ世代をそのまま返す（新しい世代を作らない）。
    #[test]
    fn lookup_after_expire_returns_dead_generation() {
        let mut reg = DmRegistry::default();
        let a = reg.register(0x10);
        reg.expire(0x10);
        let seen = reg.lookup(0x10).unwrap();
        assert_eq!(seen, a);
        assert!(!reg.is_live(seen));
    }

    /// 未登録の ptr への Uninit は死んだ slot を作り、続く通知は死んだ世代に解決される。
    #[test]
    fn expire_of_unknown_pointer_records_dead_slot() {
        let mut reg = DmRegistry::default();
        let (dead, expired) = reg.expire(0x30);
        assert!(!expired);
        assert_eq!(reg.dead_count(), 1);
        assert_eq!(reg.lookup(0x30), Some(dead));
        assert!(!reg.is_live(dead));
        // 生存中として登録し直すには新しい登録が必要で、世代は進む
        let (again, fresh) = reg.ensure_live(0x30);
        assert!(fresh);
        assert!(again.generation > dead.generation);
        assert!(reg.is_live(again));
        assert!(!reg.is_live(dead));
    }

    /// 列挙による登録は生存中の ptr の世代を進めない。死んだ slot の ptr は新しい世代になる。
    #[test]
    fn ensure_live_keeps_generation_of_live_pointer() {
        let mut reg = DmRegistry::default();
        let a = reg.register(0x10);
        let (same, fresh) = reg.ensure_live(0x10);
        assert_eq!(same, a);
        assert!(!fresh);
        assert!(reg.is_live(a));

        reg.expire(0x10);
        let (renewed, fresh) = reg.ensure_live(0x10);
        assert!(fresh);
        assert_ne!(renewed, a);
        assert!(reg.is_live(renewed));
        assert!(!reg.is_live(a));
        assert_eq!(reg.dead_count(), 0);

        // 未知の ptr は新規登録
        let (new, fresh) = reg.ensure_live(0x40);
        assert!(fresh);
        assert!(reg.is_live(new));
    }

    /// Deactivate → DM 破棄（通知は届かない）→ 再 Activate。
    #[test]
    fn expire_all_invalidates_refs_from_before_deactivate() {
        let mut reg = DmRegistry::default();
        let a = reg.register(0x10);
        let b = reg.register(0x20);
        let mut expired = reg.expire_all();
        expired.sort_by_key(|d| d.generation);
        assert_eq!(expired, vec![a, b]);
        assert!(!reg.is_live(a));
        assert!(!reg.is_live(b));
        assert_eq!(reg.live_count(), 0);
        // 再 Activate: 0x10 は残っていて列挙に現れ、0x20 は破棄されて現れない
        let (a2, fresh) = reg.ensure_live(0x10);
        assert!(fresh);
        assert!(reg.is_live(a2));
        // 世代番号は戻らない: Deactivate 前の DmRef と衝突しない
        assert!(a2.generation > b.generation);
        assert!(!reg.is_live(a));
        assert!(!reg.is_live(b));
        // 破棄された 0x20 への古い通知は死んだ世代に解決される
        assert_eq!(reg.lookup(0x20), Some(b));
    }

    #[test]
    fn dead_slots_are_evicted_oldest_first_over_cap() {
        let mut reg = DmRegistry::default();
        let cap = DmRegistry::DEAD_SLOT_CAP;
        let mut refs = Vec::new();
        for i in 0..(cap + 3) {
            let ptr = 0x1000 + i * 0x10;
            refs.push(reg.register(ptr));
            reg.expire(ptr);
        }
        assert_eq!(reg.dead_count(), cap);
        // 最も古い 3 つが捨てられ、未知の ptr になる
        for dm in &refs[..3] {
            assert_eq!(reg.lookup(dm.ptr), None);
            assert!(!reg.is_live(*dm));
        }
        assert_eq!(reg.lookup(refs[3].ptr), Some(refs[3]));
        // 捨てられたアドレスが再利用されても新しい世代として登録される
        let reused = reg.register(refs[0].ptr);
        assert!(reg.is_live(reused));
        assert!(!reg.is_live(refs[0]));
    }

    /// Uninit → 掃除で slot が消える → 古い OnSetFocus(A): 目撃しても登録されない。
    #[test]
    fn stale_focus_after_eviction_is_not_registered() {
        let mut reg = DmRegistry::default();
        let a = reg.register(0x1);
        reg.expire(0x1);
        for i in 0..DmRegistry::DEAD_SLOT_CAP {
            let ptr = 0x1000 + i * 0x10;
            reg.register(ptr);
            reg.expire(ptr);
        }
        // A の死んだ slot は掃除されている
        assert_eq!(reg.lookup(a.ptr), None);
        // 古い OnSetFocus(prev=A / next=A) の目撃: 登録されず、生存と判定されない
        assert_eq!(reg.lookup(a.ptr), None);
        assert!(!reg.is_live(a));
        assert_eq!(reg.live_count(), 0);
    }

    /// 生存中の slot は上限の対象外。
    #[test]
    fn live_slots_are_never_evicted() {
        let mut reg = DmRegistry::default();
        let live = reg.register(0x1);
        for i in 0..(DmRegistry::DEAD_SLOT_CAP + 5) {
            let ptr = 0x1000 + i * 0x10;
            reg.register(ptr);
            reg.expire(ptr);
        }
        assert!(reg.is_live(live));
        assert_eq!(reg.dead_count(), DmRegistry::DEAD_SLOT_CAP);
    }
}
