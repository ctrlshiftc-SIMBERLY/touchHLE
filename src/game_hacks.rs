/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! App-specific workarounds that replace guest methods with host code.
//!
//! Dream Dresses (com.breaktime.fashion): the game's servers are long gone.
//! Every HTTP request is made to fail promptly (as if offline), which makes
//! the game fall back to the data bundled in the app.

use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::foundation::ns_run_loop::add_perform_request;
use crate::mem::Ptr;
use crate::objc::{blocks, id, msg, msg_class, nil, ObjC, SEL};
use crate::Environment;
use std::sync::atomic::{AtomicU32, Ordering};

fn override_method(
    env: &mut Environment,
    class_name: &str,
    selector: &str,
    imp: &'static dyn crate::objc::HostIMP,
) {
    let Some(sel) = env.objc.lookup_selector(selector) else {
        log!("game_hacks: no selector {}", selector);
        return;
    };
    let class = env.objc.get_known_class(class_name, &mut env.mem);
    env.objc.override_method_with_host(class, sel, imp);
    log!("game_hacks: overrode -[{} {}]", class_name, selector);
}

/// Replacement for `-[BTHttpClient prepareRequest:withResponse:withSaveFile:]`.
/// The response handler is `^(NSDictionary *json, NSError *error)`.
fn bt_prepare_request(
    env: &mut Environment,
    _this: id,
    _cmd: SEL,
    _request: id,
    handler: id,
    _save_file: id,
) {
    log!("game_hacks: failing an HTTP request (offline mode)");
    if handler == nil {
        return;
    }
    let handler: id = blocks::block_copy(env, handler.cast_const().cast()).cast();
    let domain =
        crate::frameworks::foundation::ns_string::get_static_str(env, "com.breaktime.http");
    let error: id = msg_class![env; NSError errorWithDomain:domain code:500 userInfo:nil];
    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let sel = env
        .objc
        .lookup_selector("_touchHLE_invokeWithEmptyDictionaryAndObject:")
        .unwrap();
    add_perform_request(env, run_loop, handler, sel, error, Some(0.1), false);
}

/// Replacement for `-[StaticDataManager loadNeighborData]`: neighbors come
/// from the (dead) server, and the original never finishes when offline.
fn bt_load_neighbor_data(_env: &mut Environment, _this: id, _cmd: SEL) {
    log!("game_hacks: skipping neighbor data (offline mode)");
}

/// Replacement for `-[PlayerData secondaryCurrency]` (gems): gems can no
/// longer be bought, so make them unlimited.
fn bt_unlimited_gems(_env: &mut Environment, _this: id, _cmd: SEL) -> i32 {
    99_999
}

static ORIG_IS_PREPARED: AtomicU32 = AtomicU32::new(0);

fn orig(cell: &AtomicU32) -> GuestFunction {
    GuestFunction::from_addr_with_thumb_bit(cell.load(Ordering::Relaxed))
}

fn is_prepared(env: &mut Environment, asm: id) -> bool {
    let sel = env.objc.lookup_selector("isPrepared").unwrap();
    let r: u32 = orig(&ORIG_IS_PREPARED).call_from_host(env, (asm, sel));
    (r & 0xff) != 0
}

/// Debugging aid for characters (`TOUCHHLE_NPC_DEBUG`): logs the state of
/// an NPC's sprites periodically.
static ORIG_NPC_UPDATE: AtomicU32 = AtomicU32::new(0);
static DEBUG_COUNTER: AtomicU32 = AtomicU32::new(0);

fn read_ivar_u8(env: &mut Environment, obj: id, offset_var: u32) -> u8 {
    let off: u32 = env.mem.read(Ptr::<u32, true>::from_bits(offset_var));
    env.mem
        .read(Ptr::<u8, true>::from_bits(obj.to_bits() + off))
}
fn read_ivar_u32(env: &mut Environment, obj: id, offset_var: u32) -> u32 {
    let off: u32 = env.mem.read(Ptr::<u32, true>::from_bits(offset_var));
    env.mem
        .read(Ptr::<u32, true>::from_bits(obj.to_bits() + off))
}

fn bt_npc_update_debug(env: &mut Environment, this: id, cmd: SEL, dt: f32) {
    if DEBUG_COUNTER.fetch_add(1, Ordering::Relaxed) % 60 == 0 {
        let asm: id = Ptr::from_bits(read_ivar_u32(env, this, 0x4b644c));
        let prepared = asm != nil && is_prepared(env, asm);
        let a = read_ivar_u8(env, this, 0x4b6434);
        let b = read_ivar_u8(env, this, 0x4b6454);
        log!(
            "NPCDEBUG {:?} asm={:?} prepared={} flagA={} flagB={}",
            this,
            asm,
            prepared,
            a,
            b
        );
        if asm != nil {
            let root: id = msg![env; asm rootNode];
            let parent: id = msg![env; root parent];
            let children: id = msg![env; root children];
            let n: u32 = if children == nil {
                0
            } else {
                msg![env; children count]
            };
            let vis: bool = msg![env; root visible];
            let this_vis: bool = msg![env; this visible];
            let this_parent: id = msg![env; this parent];
            log!(
                "NPCDEBUG root={:?} root.parent={:?} children={} root.visible={} npc.visible={} npc.parent={:?}",
                root, parent, n, vis, this_vis, this_parent
            );
            for i in 0..n.min(40) {
                let c: id = msg![env; children objectAtIndex:i];
                let cv: bool = msg![env; c visible];
                let op: u8 = msg![env; c opacity];
                let sx: f32 = msg![env; c scaleX];
                let tex: id = msg![env; c texture];
                let cs: crate::frameworks::core_graphics::CGSize = msg![env; c contentSize];
                log!(
                    "NPCDEBUG  child {} vis={} op={} sx={} tex={:?} size={:?}",
                    i,
                    cv,
                    op,
                    sx,
                    tex,
                    cs
                );
                let gc: id = msg![env; c children];
                let gn: u32 = if gc == nil { 0 } else { msg![env; gc count] };
                log!("NPCDEBUG   grandchildren={}", gn);
                for j in 0..gn.min(40) {
                    let g: id = msg![env; gc objectAtIndex:j];
                    let gv: bool = msg![env; g visible];
                    let gop: u8 = msg![env; g opacity];
                    let gsx: f32 = msg![env; g scaleX];
                    let gsy: f32 = msg![env; g scaleY];
                    let gtex: id = msg![env; g texture];
                    let gcs: crate::frameworks::core_graphics::CGSize = msg![env; g contentSize];
                    let gp: crate::frameworks::core_graphics::CGPoint = msg![env; g position];
                    let cls = ObjC::read_isa(g, &env.mem);
                    let cname = env.objc.get_class_name(cls).to_string();
                    log!(
                        "NPCDEBUG   gc {} {} vis={} op={} s={},{} tex={:?} size={:?} pos={:?}",
                        j,
                        cname,
                        gv,
                        gop,
                        gsx,
                        gsy,
                        gtex,
                        gcs,
                        gp
                    );
                }
            }
        }
    }
    () = orig(&ORIG_NPC_UPDATE).call_from_host(env, (this, cmd, dt));
}

fn save_orig(env: &mut Environment, class_name: &str, selector: &str, cell: &AtomicU32) -> bool {
    let Some(sel) = env.objc.lookup_selector(selector) else {
        return false;
    };
    let class = env.objc.get_known_class(class_name, &mut env.mem);
    match env.objc.class_find_guest_imp(class, sel) {
        Ok(Some(imp)) => {
            cell.store(imp.addr_with_thumb_bit(), Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

/// Replacement for `-[PlayerData changeSecondaryCurrency:]`: gems never run
/// out, so spending always succeeds. The ivar is also kept topped up, since
/// some code reads it directly.
fn bt_change_gems(env: &mut Environment, this: id, _cmd: SEL, _amount: i32) -> bool {
    const SECONDARY_CURRENCY_IVAR_OFFSET_VAR: u32 = 0x4b6118;
    let off: u32 = env
        .mem
        .read(Ptr::<u32, true>::from_bits(SECONDARY_CURRENCY_IVAR_OFFSET_VAR));
    env.mem
        .write(Ptr::<i32, true>::from_bits(this.to_bits() + off), 99_999);
    true
}

static ORIG_IS_ACTIVE: AtomicU32 = AtomicU32::new(0);
static ORIG_DRESSES_SORTED: AtomicU32 = AtomicU32::new(0);

/// Replacement for `-[StoreItemData isActive]`: items whose artwork is
/// missing (and the buyable clerks) are treated as no longer on sale, so they
/// don't show up in the shop.
fn bt_store_item_is_active(env: &mut Environment, this: id, cmd: SEL) -> bool {
    let asset_id: id = msg![env; this getAssetId];
    if asset_id != nil {
        let name = crate::frameworks::foundation::ns_string::to_rust_string(env, asset_id);
        if crate::game_hacks_hidden::HIDDEN_STORE_ASSETS.contains(&&*name) {
            return false;
        }
    }
    let r: u32 = orig(&ORIG_IS_ACTIVE).call_from_host(env, (this, cmd));
    (r & 0xff) != 0
}

/// Replacement for `-[DressDatabase sorted]`: leaves out dresses whose
/// artwork or supplies are missing, so they're not in the sewing list.
fn bt_dresses_sorted(env: &mut Environment, this: id, cmd: SEL) -> id {
    let all: id = orig(&ORIG_DRESSES_SORTED).call_from_host(env, (this, cmd));
    if all == nil {
        return nil;
    }
    let res: id = msg_class![env; NSMutableArray array];
    let count: u32 = msg![env; all count];
    for i in 0..count {
        let dress: id = msg![env; all objectAtIndex:i];
        let dress_id: id = msg![env; dress Id];
        let hidden = dress_id != nil && {
            let name = crate::frameworks::foundation::ns_string::to_rust_string(env, dress_id);
            crate::game_hacks_hidden::HIDDEN_DRESSES.contains(&&*name)
        };
        if !hidden {
            () = msg![env; res addObject:dress];
        }
    }
    res
}

static ORIG_INGREDIENTS_GET_DATA: AtomicU32 = AtomicU32::new(0);

/// Replacement for `-[IngredientDatabase getData]` (the supplies shop list):
/// leaves out supplies whose artwork is missing.
fn bt_ingredients_get_data(env: &mut Environment, this: id, cmd: SEL) -> id {
    let all: id = orig(&ORIG_INGREDIENTS_GET_DATA).call_from_host(env, (this, cmd));
    let array_class: crate::objc::Class = msg_class![env; NSArray class];
    if all == nil || !msg![env; all isKindOfClass:array_class] {
        return all;
    }
    let res: id = msg_class![env; NSMutableArray array];
    let count: u32 = msg![env; all count];
    for i in 0..count {
        let item: id = msg![env; all objectAtIndex:i];
        let item_id: id = msg![env; item ID];
        let hidden = item_id != nil && {
            let name = crate::frameworks::foundation::ns_string::to_rust_string(env, item_id);
            crate::game_hacks_hidden::HIDDEN_INGREDIENTS.contains(&&*name)
        };
        if !hidden {
            () = msg![env; res addObject:item];
        }
    }
    res
}

/// Replacements for `-[ClerkData skin]`/`outfit`: the buyable clerks' looks
/// were downloaded content, so any hired clerk uses the default clerk's look.
fn bt_clerk_skin(env: &mut Environment, _this: id, _cmd: SEL) -> id {
    crate::frameworks::foundation::ns_string::get_static_str(env, "NPC_German")
}
fn bt_clerk_outfit(env: &mut Environment, _this: id, _cmd: SEL) -> id {
    crate::frameworks::foundation::ns_string::get_static_str(env, "C_Clerk")
}

fn bt_skip_tutorial(_env: &mut Environment, _this: id, _cmd: SEL, _t: id) {}

pub fn apply(env: &mut Environment) {
    if env.bundle.bundle_identifier() != "com.breaktime.fashion" {
        return;
    }
    override_method(
        env,
        "BTHttpClient",
        "prepareRequest:withResponse:withSaveFile:",
        &(bt_prepare_request as fn(&mut Environment, id, SEL, id, id, id)),
    );
    if std::env::var_os("TOUCHHLE_NPC_DEBUG").is_some()
        && save_orig(env, "IsoNPC", "update:", &ORIG_NPC_UPDATE)
        && save_orig(
            env,
            "AnimationStateMachine",
            "isPrepared",
            &ORIG_IS_PREPARED,
        )
    {
        override_method(
            env,
            "IsoNPC",
            "update:",
            &(bt_npc_update_debug as fn(&mut Environment, id, SEL, f32)),
        );
    }
    if std::env::var_os("TOUCHHLE_SKIP_TUTORIAL").is_some() {
        override_method(
            env,
            "TutorialManager",
            "startTutorial:",
            &(bt_skip_tutorial as fn(&mut Environment, id, SEL, id)),
        );
    }
    override_method(
        env,
        "PlayerData",
        "secondaryCurrency",
        &(bt_unlimited_gems as fn(&mut Environment, id, SEL) -> i32),
    );
    override_method(
        env,
        "PlayerData",
        "changeSecondaryCurrency:",
        &(bt_change_gems as fn(&mut Environment, id, SEL, i32) -> bool),
    );
    if save_orig(env, "StoreItemData", "isActive", &ORIG_IS_ACTIVE) {
        override_method(
            env,
            "StoreItemData",
            "isActive",
            &(bt_store_item_is_active as fn(&mut Environment, id, SEL) -> bool),
        );
    }
    if save_orig(env, "DressDatabase", "sorted", &ORIG_DRESSES_SORTED) {
        override_method(
            env,
            "DressDatabase",
            "sorted",
            &(bt_dresses_sorted as fn(&mut Environment, id, SEL) -> id),
        );
    }
    if save_orig(env, "IngredientDatabase", "getData", &ORIG_INGREDIENTS_GET_DATA) {
        override_method(
            env,
            "IngredientDatabase",
            "getData",
            &(bt_ingredients_get_data as fn(&mut Environment, id, SEL) -> id),
        );
    }
    override_method(
        env,
        "ClerkData",
        "skin",
        &(bt_clerk_skin as fn(&mut Environment, id, SEL) -> id),
    );
    override_method(
        env,
        "ClerkData",
        "outfit",
        &(bt_clerk_outfit as fn(&mut Environment, id, SEL) -> id),
    );
    override_method(
        env,
        "StaticDataManager",
        "loadNeighborData",
        &(bt_load_neighbor_data as fn(&mut Environment, id, SEL)),
    );
}
