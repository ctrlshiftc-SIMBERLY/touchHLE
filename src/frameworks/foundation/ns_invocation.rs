/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSInvocation`.

use crate::abi::{extend_stack_for_args, write_next_arg, GuestArg};
use crate::cpu::Cpu;
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::libc::string::strdup;
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr};
use crate::msg;
use crate::objc::{
    autorelease, id, nil, objc_classes, objc_msgSend, release, retain, ClassExports, HostObject,
    SEL,
};

struct NSInvocationHostObject {
    /// `NSMethodSignature *`
    sig: id,
    /// Argument type strings resolved from `sig` at creation time
    argument_types: Vec<String>,
    target: id,
    selector: Option<SEL>,
    /// Per-slot owned buffer for each argument.
    /// Option denotes if argument was set with `setArgument:atIndex:`
    arguments: Vec<Option<MutVoidPtr>>,
    arguments_retained: bool,
    /// Objects retained by `retainArguments`
    retained_objects: Vec<id>,
    /// C string copies made by `retainArguments`
    copied_strings: Vec<MutPtr<u8>>,
}
impl HostObject for NSInvocationHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSInvocation: NSObject

+ (id)invocationWithMethodSignature:(id)sig { // NSMethodSignature *
    retain(env, sig);
    let num_of_args: NSUInteger = msg![env; sig numberOfArguments];
    let mut argument_types: Vec<String> = Vec::with_capacity(num_of_args as usize);
    for i in 0..num_of_args {
        let type_ptr: ConstPtr<u8> = msg![env; sig getArgumentTypeAtIndex:i];
        argument_types.push(env.mem.cstr_at_utf8(type_ptr).unwrap().to_string());
    }
    let host_object = Box::new(NSInvocationHostObject {
        sig,
        argument_types,
        target: nil,
        selector: None,
        arguments: vec![None; num_of_args as usize],
        arguments_retained: false,
        retained_objects: Vec::new(),
        copied_strings: Vec::new(),
    });
    let res = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, res)
}

- (id)target {
    env.objc.borrow::<NSInvocationHostObject>(this).target
}
- (())setTarget:(id)target {
    let old_target = env.objc.borrow::<NSInvocationHostObject>(this).target;
    let arguments_retained = env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained;
    env.objc.borrow_mut::<NSInvocationHostObject>(this).target = target;
    if arguments_retained {
        retain(env, target);
        release(env, old_target);
    }
}

- (())setSelector:(SEL)selector {
    assert!(env.objc.borrow_mut::<NSInvocationHostObject>(this).selector.is_none()); // TODO
    env.objc.borrow_mut::<NSInvocationHostObject>(this).selector = Some(selector);
}

- (())retainArguments {
    // TODO: handle return val
    // TODO: copy blocks
    assert!(!env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained); // TODO

    let target = env.objc.borrow::<NSInvocationHostObject>(this).target;
    retain(env, target);

    let mut retained_objects: Vec<id> = Vec::new();
    let mut copied_strings: Vec<MutPtr<u8>> = Vec::new();

    // Skip index 0 (self) and 1 (SEL): handled via target/selector fields.
    let num_of_args = env.objc.borrow::<NSInvocationHostObject>(this).argument_types.len();
    for i in 2..num_of_args {
        let host = env.objc.borrow::<NSInvocationHostObject>(this);
        let Some(arg_loc) = host.arguments[i] else { continue };
        match host.argument_types[i].as_str() {
            "@" => {
                let obj: id = env.mem.read(arg_loc.cast().cast_const());
                retain(env, obj);
                retained_objects.push(obj);
            }
            "*" => {
                let str: MutPtr<u8> = env.mem.read(arg_loc.cast().cast_const());
                let str_copy = strdup(env, str.cast_const());
                env.mem.write(arg_loc.cast(), str_copy);
                copied_strings.push(str_copy);
            }
            _ => {}
        }
    }

    let host = env.objc.borrow_mut::<NSInvocationHostObject>(this);
    host.retained_objects = retained_objects;
    host.copied_strings = copied_strings;
    host.arguments_retained = true;
}

- (())setArgument:(MutVoidPtr)arg_loc
          atIndex:(NSInteger)idx {
    let &NSInvocationHostObject {
        ref arguments,
        arguments_retained,
        ..
    } = env.objc.borrow::<NSInvocationHostObject>(this);

    // 0 and 1 are reserved for `self` and `_cmd`
    // TODO: can they be set too?
    assert!(1 < idx && idx < arguments.len() as NSInteger);

    if let Some(prev_arg) = arguments[idx as usize] {
        env.mem.free(prev_arg.cast());
    }

    let _ = arguments_retained;
    let arg_type = env.objc.borrow::<NSInvocationHostObject>(this).argument_types[idx as usize].clone();
    let size = arg_size(arg_type.as_bytes()).max(1);
    let new: MutVoidPtr = env.mem.alloc(size);
    let bytes = env.mem.bytes_at(arg_loc.cast(), size).to_vec();
    env.mem.bytes_at_mut(new.cast(), size).copy_from_slice(&bytes);
    env.objc.borrow_mut::<NSInvocationHostObject>(this).arguments[idx as usize] = Some(new);
}

- (())invokeWithTarget:(id)target {
    () = msg![env; this setTarget:target];
    () = msg![env; this invoke];
}

- (())invoke {
    let sig = env.objc.borrow::<NSInvocationHostObject>(this).sig;
    let ret_type: ConstPtr<u8> = msg![env; sig methodReturnType];
    let ret_char = env.mem.read(ret_type);

    // Flatten all arguments into 32-bit words following the ARM AAPCS
    // (soft-float): everything goes into r0-r3 then the stack, 64-bit values
    // are aligned to an even word.
    let mut words: Vec<u32> = Vec::new();
    let &NSInvocationHostObject { target, selector, .. } = env.objc.borrow::<NSInvocationHostObject>(this);
    let selector = selector.unwrap();
    words.push(target.to_bits());
    let sel_words: [u32; 1] = { let mut r = [0u32; 1]; <SEL as GuestArg>::to_regs(selector, &mut r); r };
    words.push(sel_words[0]);
    let host = env.objc.borrow::<NSInvocationHostObject>(this);
    let args: Vec<(String, Option<MutVoidPtr>)> = host
        .argument_types
        .iter()
        .cloned()
        .zip(host.arguments.iter().copied())
        .skip(2)
        .collect();
    for (ty, slot) in args {
        let size = arg_size(ty.as_bytes());
        let first = ty.trim_start_matches(|c: char| "rnNoORV".contains(c)).bytes().next();
        if matches!(first, Some(b'q' | b'Q' | b'd')) && words.len() % 2 == 1 {
            words.push(0);
        }
        let n = size.div_ceil(4).max(1) as usize;
        let mut buf = vec![0u8; n * 4];
        if let Some(slot) = slot {
            let bytes = env.mem.bytes_at(slot.cast(), size).to_vec();
            buf[..bytes.len()].copy_from_slice(&bytes);
        }
        // sub-word integer types are extended to a full word
        match first {
            Some(b'c') => { let v = (buf[0] as i8 as i32).to_le_bytes(); buf[..4].copy_from_slice(&v) }
            Some(b's') => { let v = (i16::from_le_bytes([buf[0], buf[1]]) as i32).to_le_bytes(); buf[..4].copy_from_slice(&v) }
            _ => {}
        }
        for w in buf.chunks(4) {
            words.push(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        }
    }

    let regs = env.cpu.regs_mut();
    let old_sp = extend_stack_for_args(words.len(), regs);
    let mut reg_offset = 0;
    for w in words {
        let regs = env.cpu.regs_mut();
        write_next_arg::<u32>(&mut reg_offset, regs, &mut env.mem, w);
    }

    if ret_char == b'{' {
        log!("TODO: NSInvocation with struct return, ignoring return value");
    }
    objc_msgSend(env, target, selector);

    let regs = env.cpu.regs_mut(); // re-borrow
    regs[Cpu::SP] = old_sp;
}

- (())dealloc {
    let &NSInvocationHostObject { sig, target, arguments_retained, .. } = env.objc.borrow::<NSInvocationHostObject>(this);
    release(env, sig);
    if arguments_retained {
        release(env, target);
        let retained_objects = std::mem::take(
            &mut env.objc.borrow_mut::<NSInvocationHostObject>(this).retained_objects
        );
        for obj in retained_objects {
            release(env, obj);
        }
        let copied_strings = std::mem::take(
            &mut env.objc.borrow_mut::<NSInvocationHostObject>(this).copied_strings
        );
        for s in copied_strings {
            env.mem.free(s.cast());
        }
    } else {
        assert!(env.objc.borrow::<NSInvocationHostObject>(this).retained_objects.is_empty());
        assert!(env.objc.borrow::<NSInvocationHostObject>(this).copied_strings.is_empty());
    }
    for ptr in env.objc.borrow::<NSInvocationHostObject>(this).arguments.iter().flatten() {
        env.mem.free(ptr.cast());
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

fn arg_size(ty: &[u8]) -> u32 {
    crate::frameworks::foundation::ns_value::objc_type_size(ty).0
}
