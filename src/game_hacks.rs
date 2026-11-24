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

use crate::frameworks::foundation::ns_run_loop::add_perform_request;
use crate::objc::{blocks, id, msg_class, nil, SEL};
use crate::Environment;

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
    override_method(
        env,
        "StaticDataManager",
        "loadNeighborData",
        &(bt_load_neighbor_data as fn(&mut Environment, id, SEL)),
    );
}
