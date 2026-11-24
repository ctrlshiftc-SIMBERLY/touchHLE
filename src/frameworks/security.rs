/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Security framework: a keychain that is always empty.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, MutPtr, Ptr};
use crate::Environment;

const ERR_SEC_SUCCESS: i32 = 0;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

fn SecItemCopyMatching(
    env: &mut Environment,
    _query: ConstVoidPtr,
    result: MutPtr<ConstVoidPtr>,
) -> i32 {
    if !result.is_null() {
        env.mem.write(result, Ptr::null());
    }
    ERR_SEC_ITEM_NOT_FOUND
}
fn SecItemAdd(
    env: &mut Environment,
    _attributes: ConstVoidPtr,
    result: MutPtr<ConstVoidPtr>,
) -> i32 {
    if !result.is_null() {
        env.mem.write(result, Ptr::null());
    }
    ERR_SEC_SUCCESS
}
fn SecItemUpdate(_env: &mut Environment, _query: ConstVoidPtr, _attributes: ConstVoidPtr) -> i32 {
    ERR_SEC_SUCCESS
}
fn SecItemDelete(_env: &mut Environment, _query: ConstVoidPtr) -> i32 {
    ERR_SEC_SUCCESS
}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/Security.framework/Security",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

const FUNCTIONS: FunctionExports = &[
    export_c_func!(SecItemCopyMatching(_, _)),
    export_c_func!(SecItemAdd(_, _)),
    export_c_func!(SecItemUpdate(_, _)),
    export_c_func!(SecItemDelete(_)),
];
