/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Blocks runtime (iOS 4+) and ARC runtime entry points.
//!
//! Reference: <https://clang.llvm.org/docs/Block-ABI-Apple.html>
//!
//! This is a simplified implementation: heap blocks and heap `__block`
//! variables are never freed. Blocks are given real Objective-C classes so
//! that sending them `retain`/`release`/`copy` etc works.

use super::{autorelease, id, msg_class, objc_classes, release, retain, ClassExports};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;

const BLOCK_NEEDS_FREE: u32 = 1 << 24;
const BLOCK_HAS_COPY_DISPOSE: u32 = 1 << 25;
const BLOCK_IS_GLOBAL: u32 = 1 << 28;

const BLOCK_FIELD_IS_OBJECT: u32 = 3;
const BLOCK_FIELD_IS_BLOCK: u32 = 7;
const BLOCK_FIELD_IS_BYREF: u32 = 8;
const BLOCK_BYREF_CALLER: u32 = 128;

fn malloc_block_class(env: &mut Environment) -> u32 {
    env.objc
        .get_known_class("__NSMallocBlock__", &mut env.mem)
        .to_bits()
}

/// Calls a block that takes no arguments and returns nothing.
pub fn call_void_block(env: &mut Environment, block: ConstVoidPtr) {
    let invoke: u32 = env.mem.read(block.cast::<u32>() + 3);
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke);
    () = invoke.call_from_host(env, (block,));
}

pub fn block_copy(env: &mut Environment, block: ConstVoidPtr) -> MutVoidPtr {
    if block.is_null() {
        return Ptr::null();
    }
    let words: MutPtr<u32> = block.cast_mut().cast();
    let isa: u32 = env.mem.read(words);
    let flags: u32 = env.mem.read(words + 1);
    if isa == malloc_block_class(env) || (flags & BLOCK_IS_GLOBAL) != 0 {
        return block.cast_mut();
    }
    let descriptor: u32 = env.mem.read(words + 4);
    let descriptor: MutPtr<u32> = Ptr::from_bits(descriptor);
    let size: u32 = env.mem.read(descriptor + 1);
    let new = env.mem.alloc(size);
    env.mem.memmove(new, block, size);
    let new_words: MutPtr<u32> = new.cast();
    let malloc_class = malloc_block_class(env);
    env.mem.write(new_words, malloc_class);
    env.mem.write(new_words + 1, flags | BLOCK_NEEDS_FREE);
    if (flags & BLOCK_HAS_COPY_DISPOSE) != 0 {
        let copy_helper: u32 = env.mem.read(descriptor + 2);
        if copy_helper != 0 {
            let copy_helper = GuestFunction::from_addr_with_thumb_bit(copy_helper);
            () = copy_helper.call_from_host(env, (new, block));
        }
    }
    log_dbg!("_Block_copy({:?}) => {:?}", block, new);
    new
}

fn byref_copy(env: &mut Environment, src: MutVoidPtr) -> u32 {
    let words: MutPtr<u32> = src.cast();
    let forwarding: u32 = env.mem.read(words + 1);
    if forwarding != src.to_bits() {
        // Already moved to the heap.
        return forwarding;
    }
    let flags: u32 = env.mem.read(words + 2);
    let size: u32 = env.mem.read(words + 3);
    let new = env.mem.alloc(size);
    env.mem.memmove(new, src.cast_const(), size);
    let new_words: MutPtr<u32> = new.cast();
    env.mem.write(new_words + 1, new.to_bits());
    env.mem.write(words + 1, new.to_bits());
    if (flags & BLOCK_HAS_COPY_DISPOSE) != 0 {
        let keep: u32 = env.mem.read(words + 4);
        if keep != 0 {
            let keep = GuestFunction::from_addr_with_thumb_bit(keep);
            () = keep.call_from_host(env, (new, src));
        }
    }
    new.to_bits()
}

fn _Block_copy(env: &mut Environment, block: ConstVoidPtr) -> MutVoidPtr {
    block_copy(env, block)
}

fn _Block_release(_env: &mut Environment, _block: ConstVoidPtr) {
    // Heap blocks are leaked for simplicity.
}

fn _Block_object_assign(env: &mut Environment, dest: MutPtr<u32>, object: MutVoidPtr, flags: u32) {
    let value = if (flags & BLOCK_BYREF_CALLER) != 0 {
        // Called from a __block variable's copy helper: no retain.
        object.to_bits()
    } else if (flags & BLOCK_FIELD_IS_BYREF) == BLOCK_FIELD_IS_BYREF {
        byref_copy(env, object)
    } else if (flags & BLOCK_FIELD_IS_BLOCK) == BLOCK_FIELD_IS_BLOCK {
        block_copy(env, object.cast_const()).to_bits()
    } else if (flags & BLOCK_FIELD_IS_OBJECT) == BLOCK_FIELD_IS_OBJECT {
        retain(env, object.cast()).to_bits()
    } else {
        object.to_bits()
    };
    env.mem.write(dest, value);
}

fn _Block_object_dispose(_env: &mut Environment, _object: ConstVoidPtr, _flags: i32) {
    // Leaked for simplicity.
}

// ARC runtime functions (iOS 5+)

fn objc_retain(env: &mut Environment, obj: id) -> id {
    retain(env, obj)
}
fn objc_release(env: &mut Environment, obj: id) {
    release(env, obj)
}
fn objc_autorelease(env: &mut Environment, obj: id) -> id {
    autorelease(env, obj)
}
fn objc_autoreleaseReturnValue(env: &mut Environment, obj: id) -> id {
    autorelease(env, obj)
}
fn objc_retainAutoreleasedReturnValue(env: &mut Environment, obj: id) -> id {
    retain(env, obj)
}
fn objc_retainAutorelease(env: &mut Environment, obj: id) -> id {
    let obj = retain(env, obj);
    autorelease(env, obj)
}
fn objc_retainAutoreleaseReturnValue(env: &mut Environment, obj: id) -> id {
    let obj = retain(env, obj);
    autorelease(env, obj)
}
fn objc_storeStrong(env: &mut Environment, location: MutPtr<id>, obj: id) {
    let old = env.mem.read(location);
    if old == obj {
        return;
    }
    let obj = retain(env, obj);
    env.mem.write(location, obj);
    release(env, old);
}
fn objc_retainBlock(env: &mut Environment, block: id) -> id {
    block_copy(env, block.cast_const().cast()).cast()
}

pub const CONSTANTS: ConstantExports = &[
    // The isa of a block is the address of these symbols, so they must be the
    // classes themselves.
    (
        "__NSConcreteGlobalBlock",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            env.objc
                .get_known_class("__NSGlobalBlock__", &mut env.mem)
                .cast()
                .cast_const()
        }),
    ),
    (
        "__NSConcreteStackBlock",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            env.objc
                .get_known_class("__NSStackBlock__", &mut env.mem)
                .cast()
                .cast_const()
        }),
    ),
    (
        "__NSConcreteMallocBlock",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            env.objc
                .get_known_class("__NSMallocBlock__", &mut env.mem)
                .cast()
                .cast_const()
        }),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(_Block_copy(_)),
    export_c_func!(_Block_release(_)),
    export_c_func!(_Block_object_assign(_, _, _)),
    export_c_func!(_Block_object_dispose(_, _)),
    export_c_func!(objc_retain(_)),
    export_c_func!(objc_release(_)),
    export_c_func!(objc_autorelease(_)),
    export_c_func!(objc_autoreleaseReturnValue(_)),
    export_c_func!(objc_retainAutoreleasedReturnValue(_)),
    export_c_func!(objc_retainAutorelease(_)),
    export_c_func!(objc_retainAutoreleaseReturnValue(_)),
    export_c_func!(objc_storeStrong(_, _)),
    export_c_func!(objc_retainBlock(_)),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSBlock: NSObject

- (id)retain {
    this
}
- (())release {
}
- (id)autorelease {
    this
}
- (id)copy {
    block_copy(env, this.cast_const().cast()).cast()
}
- (id)copyWithZone:(MutVoidPtr)_zone {
    block_copy(env, this.cast_const().cast()).cast()
}
- (())invoke {
    call_void_block(env, this.cast_const().cast());
}
// Used by app-specific workarounds: calls a `^(id, id)` block with an empty
// NSDictionary and the given object.
- (())_touchHLE_invokeWithEmptyDictionaryAndObject:(id)object {
    let dict: id = msg_class![env; NSDictionary dictionary];
    let invoke: u32 = env.mem.read(this.cast::<u32>() + 3);
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke);
    () = invoke.call_from_host(env, (this, dict, object));
}
// Used by touchHLE's GCD implementation for deferred blocks.
- (())_touchHLE_invokeWithObject:(id)_ignored {
    call_void_block(env, this.cast_const().cast());
}

@end

@implementation __NSGlobalBlock__: NSBlock
@end

@implementation __NSStackBlock__: NSBlock
@end

@implementation __NSMallocBlock__: NSBlock
@end

};
