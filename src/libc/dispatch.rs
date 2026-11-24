/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Grand Central Dispatch (`libdispatch`), very simplified.
//!
//! Everything runs on the main thread: asynchronous work is deferred to the
//! main run loop, synchronous work runs immediately. This is enough for
//! typical game use (one-time init, "do this later on the main queue", etc).

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_run_loop::add_perform_request;
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{blocks, id, msg_class, nil};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    /// Dispatch groups: outstanding count and pending notify blocks.
    groups: HashMap<u32, (u32, Vec<(MutVoidPtr, MutVoidPtr)>)>,
    global_queue: Option<MutVoidPtr>,
}

#[allow(non_camel_case_types)]
type dispatch_queue_t = MutVoidPtr;
#[allow(non_camel_case_types)]
type dispatch_block_t = ConstVoidPtr;

fn defer_block(env: &mut Environment, block: dispatch_block_t, delay: f64) {
    let block = blocks::block_copy(env, block);
    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let sel = env
        .objc
        .lookup_selector("_touchHLE_invokeWithObject:")
        .unwrap();
    add_perform_request(env, run_loop, block.cast(), sel, nil, Some(delay), false);
}

fn dispatch_once(env: &mut Environment, predicate: MutPtr<i32>, block: dispatch_block_t) {
    if env.mem.read(predicate) != 0 {
        return;
    }
    env.mem.write(predicate, -1);
    blocks::call_void_block(env, block);
}

fn dispatch_async(env: &mut Environment, _queue: dispatch_queue_t, block: dispatch_block_t) {
    defer_block(env, block, 0.0);
}

fn dispatch_sync(env: &mut Environment, _queue: dispatch_queue_t, block: dispatch_block_t) {
    blocks::call_void_block(env, block);
}

fn dispatch_after(
    env: &mut Environment,
    when: u64, // dispatch_time_t
    _queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    // We return absolute "now + delta" from dispatch_time, in nanoseconds
    // relative to an arbitrary epoch of 0, so `when` is just the delta.
    let delay = if when == u64::MAX {
        return;
    } else {
        (when as f64) / 1e9
    };
    defer_block(env, block, delay);
}

fn dispatch_time(_env: &mut Environment, _when: u64, delta: i64) -> u64 {
    delta.max(0) as u64
}

fn dispatch_get_global_queue(env: &mut Environment, _priority: i32, _flags: u32) -> MutVoidPtr {
    if let Some(q) = env.libc_state.dispatch.global_queue {
        return q;
    }
    let q = env.mem.alloc(16);
    env.libc_state.dispatch.global_queue = Some(q);
    q
}

fn dispatch_get_current_queue(env: &mut Environment) -> MutVoidPtr {
    dispatch_get_global_queue(env, 0, 0)
}

fn dispatch_queue_create(
    env: &mut Environment,
    _label: ConstVoidPtr,
    _attr: ConstVoidPtr,
) -> MutVoidPtr {
    env.mem.alloc(16)
}

fn dispatch_retain(_env: &mut Environment, _object: MutVoidPtr) {}
fn dispatch_release(_env: &mut Environment, _object: MutVoidPtr) {}
fn dispatch_set_target_queue(_env: &mut Environment, _object: MutVoidPtr, _queue: MutVoidPtr) {}

fn dispatch_group_create(env: &mut Environment) -> MutVoidPtr {
    let g = env.mem.alloc(16);
    env.libc_state
        .dispatch
        .groups
        .insert(g.to_bits(), (0, Vec::new()));
    g
}

fn dispatch_group_enter(env: &mut Environment, group: MutVoidPtr) {
    env.libc_state
        .dispatch
        .groups
        .entry(group.to_bits())
        .or_default()
        .0 += 1;
}

fn dispatch_group_leave(env: &mut Environment, group: MutVoidPtr) {
    let entry = env
        .libc_state
        .dispatch
        .groups
        .entry(group.to_bits())
        .or_default();
    entry.0 = entry.0.saturating_sub(1);
    if entry.0 == 0 {
        let pending = std::mem::take(&mut entry.1);
        for (_queue, block) in pending {
            defer_block(env, block.cast_const(), 0.0);
        }
    }
}

fn dispatch_group_async(
    env: &mut Environment,
    group: MutVoidPtr,
    _queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    // Run immediately so the group is never left waiting.
    dispatch_group_enter(env, group);
    blocks::call_void_block(env, block);
    dispatch_group_leave(env, group);
}

fn dispatch_group_notify(
    env: &mut Environment,
    group: MutVoidPtr,
    queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    let count = env
        .libc_state
        .dispatch
        .groups
        .get(&group.to_bits())
        .map_or(0, |e| e.0);
    if count == 0 {
        defer_block(env, block, 0.0);
    } else {
        let block = blocks::block_copy(env, block);
        env.libc_state
            .dispatch
            .groups
            .get_mut(&group.to_bits())
            .unwrap()
            .1
            .push((queue, block));
    }
}

fn dispatch_group_wait(_env: &mut Environment, _group: MutVoidPtr, _timeout: u64) -> i32 {
    0
}

fn dispatch_main(_env: &mut Environment) {
    unimplemented!("dispatch_main")
}

pub const CONSTANTS: ConstantExports = &[(
    "__dispatch_main_q",
    HostConstant::Custom(|env| -> ConstVoidPtr { env.mem.alloc(64).cast_const() }),
)];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(dispatch_once(_, _)),
    export_c_func!(dispatch_async(_, _)),
    export_c_func!(dispatch_sync(_, _)),
    export_c_func!(dispatch_after(_, _, _)),
    export_c_func!(dispatch_time(_, _)),
    export_c_func!(dispatch_get_global_queue(_, _)),
    export_c_func!(dispatch_get_current_queue()),
    export_c_func!(dispatch_queue_create(_, _)),
    export_c_func!(dispatch_retain(_)),
    export_c_func!(dispatch_release(_)),
    export_c_func!(dispatch_set_target_queue(_, _)),
    export_c_func!(dispatch_group_create()),
    export_c_func!(dispatch_group_enter(_)),
    export_c_func!(dispatch_group_leave(_)),
    export_c_func!(dispatch_group_async(_, _, _)),
    export_c_func!(dispatch_group_notify(_, _, _)),
    export_c_func!(dispatch_group_wait(_, _)),
    export_c_func!(dispatch_main()),
];
