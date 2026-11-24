/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `NSValue` class cluster, including `NSNumber`.

use super::ns_string::{from_rust_ordering, from_rust_string};
use super::{
    _nib_archive_decoder, ns_keyed_unarchiver, NSComparisonResult, NSOrderedSame, NSUInteger,
};
use crate::frameworks::core_foundation::cf_number::{
    kCFNumberCharType, kCFNumberFloat32Type, kCFNumberFloatType, kCFNumberIntType,
    kCFNumberSInt16Type, kCFNumberSInt32Type, kCFNumberSInt8Type, kCFNumberShortType, CFNumberType,
};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_keyed_archiver::get_value_to_encode_for_current_key;
use crate::frameworks::foundation::NSInteger;
use crate::mem::{ConstPtr, ConstVoidPtr, MutVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;
use std::cmp::Ordering;

#[derive(Debug)]
pub(super) enum NSValueHostObject {
    CGPoint(CGPoint),
    CGSize(CGSize),
    CGRect(CGRect),
    /// Generic value from `valueWithBytes:objCType:`.
    Bytes(Vec<u8>),
}
impl HostObject for NSValueHostObject {}

/// Computes (size, alignment) of an Objective-C type encoding, for the
/// simple cases used with NSValue. Returns the remaining unparsed string.
fn objc_type_size(enc: &[u8]) -> (u32, u32, &[u8]) {
    let mut enc = enc;
    // skip type qualifiers
    while let Some(&c) = enc.first() {
        if b"rnNoORV".contains(&c) {
            enc = &enc[1..];
        } else {
            break;
        }
    }
    let Some(&c) = enc.first() else {
        return (0, 1, enc);
    };
    let rest = &enc[1..];
    match c {
        b'c' | b'C' | b'B' => (1, 1, rest),
        b's' | b'S' => (2, 2, rest),
        b'i' | b'I' | b'l' | b'L' | b'f' | b'*' | b'@' | b'#' | b':' => (4, 4, rest),
        b'q' | b'Q' | b'd' => (8, 4, rest),
        b'^' => {
            let (_, _, rest) = objc_type_size(rest);
            (4, 4, rest)
        }
        b'{' | b'(' => {
            let close = if c == b'{' { b'}' } else { b')' };
            // skip name
            let mut r = rest;
            while let Some(&ch) = r.first() {
                if ch == b'=' {
                    r = &r[1..];
                    break;
                }
                if ch == close {
                    break;
                }
                r = &r[1..];
            }
            let (mut size, mut align) = (0u32, 1u32);
            while let Some(&ch) = r.first() {
                if ch == close {
                    r = &r[1..];
                    break;
                }
                let (s, a, rr) = objc_type_size(r);
                if rr.len() == r.len() {
                    break;
                }
                r = rr;
                align = align.max(a);
                if c == b'{' {
                    size = size.div_ceil(a) * a + s;
                } else {
                    size = size.max(s);
                }
            }
            (size.div_ceil(align) * align, align, r)
        }
        b'[' => {
            let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
            let n: u32 = std::str::from_utf8(&rest[..digits])
                .unwrap()
                .parse()
                .unwrap_or(0);
            let (s, a, mut r) = objc_type_size(&rest[digits..]);
            if r.first() == Some(&b']') {
                r = &r[1..];
            }
            (n * s, a, r)
        }
        _ => (4, 4, rest),
    }
}

macro_rules! impl_AsValue {
    ($method_name:tt, $typ:tt) => {
        pub fn $method_name(&self) -> $typ {
            match self {
                // Cast to u8 is needed for float conversions
                NSNumberHostObject::Bool(x) => *x as u8 as _,
                NSNumberHostObject::UnsignedLongLong(x) => *x as _,
                NSNumberHostObject::UnsignedInt(x) => *x as _,
                NSNumberHostObject::Int(x) => *x as _,
                NSNumberHostObject::LongLong(x) => *x as _,
                NSNumberHostObject::Float(x) => *x as _,
                NSNumberHostObject::Double(x) => *x as _,
                NSNumberHostObject::Short(x) => *x as _,
                NSNumberHostObject::UnsignedShort(x) => *x as _,
                NSNumberHostObject::Char(x) => *x as _,
            }
        }
    };
}

#[derive(Debug)]
pub(super) enum NSNumberHostObject {
    Bool(bool),
    UnsignedLongLong(u64),
    UnsignedInt(u32),
    Int(i32), // Also covers Integer and Long since this is a 32-bit platform.
    LongLong(i64),
    Float(f32),
    Double(f64),
    Short(i16),
    UnsignedShort(u16),
    Char(i8),
}
impl HostObject for NSNumberHostObject {}

impl NSNumberHostObject {
    fn as_bool(&self) -> bool {
        match self {
            NSNumberHostObject::Bool(x) => *x,
            NSNumberHostObject::UnsignedLongLong(x) => *x != 0,
            NSNumberHostObject::UnsignedInt(x) => *x != 0,
            NSNumberHostObject::Int(x) => *x != 0,
            NSNumberHostObject::LongLong(x) => *x != 0,
            NSNumberHostObject::Float(x) => *x != 0.0,
            NSNumberHostObject::Double(x) => *x != 0.0,
            NSNumberHostObject::Short(x) => *x != 0,
            NSNumberHostObject::UnsignedShort(x) => *x != 0,
            NSNumberHostObject::Char(x) => *x != 0,
        }
    }
    fn is_float(&self) -> bool {
        matches!(
            self,
            NSNumberHostObject::Float(_) | NSNumberHostObject::Double(_)
        )
    }
    impl_AsValue!(as_int, i32);
    impl_AsValue!(as_long_long, i64);
    impl_AsValue!(as_unsigned_long_long, u64);
    impl_AsValue!(as_unsigned_int, u32);
    impl_AsValue!(as_float, f32);
    impl_AsValue!(as_double, f64);
    impl_AsValue!(as_short, i16);
    impl_AsValue!(as_unsigned_short, u16);
    impl_AsValue!(as_char, i8);
    impl_AsValue!(as_i128, i128);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// NSValue is an abstract class. None of the things it should provide are
// implemented here yet (TODO).
@implementation NSValue: NSObject

+ (id)valueWithPointer:(ConstVoidPtr)ptr {
    // TODO: implement with `value:withObjCType:` instead
    msg_class![env; NSNumber numberWithUnsignedInt:(ptr.to_bits())]
}

+ (id)valueWithBytes:(ConstVoidPtr)value objCType:(ConstPtr<u8>)type_ {
    let enc = env.mem.cstr_at(type_).to_vec();
    let (size, _, _) = objc_type_size(&enc);
    let bytes = env.mem.bytes_at(value.cast(), size).to_vec();
    let host_object = Box::new(NSValueHostObject::Bytes(bytes));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)value:(ConstVoidPtr)value withObjCType:(ConstPtr<u8>)type_ {
    msg![env; this valueWithBytes:value objCType:type_]
}

+ (id)valueWithCGPoint:(CGPoint)value {
    let host_object = Box::new(NSValueHostObject::CGPoint(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)valueWithCGSize:(CGSize)value {
    let host_object = Box::new(NSValueHostObject::CGSize(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)valueWithCGRect:(CGRect)value {
    let host_object = Box::new(NSValueHostObject::CGRect(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

- (())getValue:(MutVoidPtr)buffer {
    let bytes: Vec<u8> = match env.objc.borrow::<NSValueHostObject>(this) {
        NSValueHostObject::Bytes(b) => b.clone(),
        NSValueHostObject::CGPoint(p) => [p.x.to_le_bytes(), p.y.to_le_bytes()].concat(),
        NSValueHostObject::CGSize(p) => [p.width.to_le_bytes(), p.height.to_le_bytes()].concat(),
        NSValueHostObject::CGRect(r) => [
            r.origin.x.to_le_bytes(),
            r.origin.y.to_le_bytes(),
            r.size.width.to_le_bytes(),
            r.size.height.to_le_bytes(),
        ]
        .concat(),
    };
    env.mem
        .bytes_at_mut(buffer.cast(), bytes.len() as u32)
        .copy_from_slice(&bytes);
}

- (CGPoint)CGPointValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGPoint(cg_point) => *cg_point,
        _ => unimplemented!()
    }
}

- (CGSize)CGSizeValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGSize(cg_size) => *cg_size,
        _ => unimplemented!()
    }
}

- (CGRect)CGRectValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGRect(cg_rect) => *cg_rect,
        _ => unimplemented!()
    }
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (MutVoidPtr)pointerValue {
    let class: Class = msg![env; this class];
    assert!(class == env.objc.get_known_class("NSNumber", &mut env.mem));
    // According to the docs, `If the value object was not created to hold
    // a pointer-sized data item, the result is undefined.`
    let val = msg![env; this unsignedIntValue];
    MutVoidPtr::from_bits(val)
}

@end

// NSNumber is not an abstract class.
@implementation NSNumber: NSValue

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSNumberHostObject::Bool(false));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)numberWithBool:(bool)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithBool:value];
    autorelease(env, new)
}

+ (id)numberWithFloat:(f32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithFloat:value];
    autorelease(env, new)
}

+ (id)numberWithDouble:(f64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithDouble:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedInt:(u32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedInt:value];
    autorelease(env, new)
}

+ (id)numberWithInt:(i32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithInt:value];
    autorelease(env, new)
}

+ (id)numberWithLong:(i32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithLong:value];
    autorelease(env, new)
}

+ (id)numberWithInteger:(NSInteger)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithInteger:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedInteger:(NSUInteger)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedInteger:value];
    autorelease(env, new)
}

+ (id)numberWithLongLong:(i64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithLongLong:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedLongLong:(u64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedLongLong:value];
    autorelease(env, new)
}

+ (id)numberWithShort:(i16)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithShort:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedShort:(u16)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedShort:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedLong:(u32)value {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedInt:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedChar:(u8)value {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedShort:(value as u16)];
    autorelease(env, new)
}

+ (id)numberWithChar:(i8)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithChar:value];
    autorelease(env, new)
}

// TODO: types other than booleans and long longs

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    let class: Class = msg![env; coder class];
    let keyed_unarch_class: Class = msg_class![env; NSKeyedUnarchiver class];
    let nib_archive_class: Class = msg_class![env; _touchHLE_NIBArchiveDecoder class];
    let new_num = if env.objc.class_is_subclass_of(class, keyed_unarch_class) {
        ns_keyed_unarchiver::decode_current_number(env, coder)
    } else if env.objc.class_is_subclass_of(class, nib_archive_class) {
        _nib_archive_decoder::decode_current_number(env, coder)
    } else {
        unimplemented!();
    };
    release(env, this);
    new_num
}
- (())encodeWithCoder:(id)coder {
    let host_object = env.objc.borrow::<NSNumberHostObject>(this);
    let (key, val) = match host_object {
        NSNumberHostObject::Int(i) => ("NS.intval", plist::Value::Integer((*i).into())),
        NSNumberHostObject::Double(d) => ("NS.dblval", plist::Value::Real(*d)),
        NSNumberHostObject::Bool(b) => ("NS.boolval", plist::Value::Boolean(*b)),
        _ => unimplemented!("{:?}", host_object)
    };

    let scope = get_value_to_encode_for_current_key(env, coder);
    scope.insert(key.to_string(), val);
}

- (id)initWithBool:(bool)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Bool(value);
    this
}

- (id)initWithFloat:(f32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Float(value);
    this
}

- (id)initWithDouble:(f64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Double(value);
    this
}

- (id)initWithLongLong:(i64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::LongLong(value);
    this
}

- (id)initWithUnsignedInt:(u32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(value);
    this
}

- (id)initWithInt:(i32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithLong:(i32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithInteger:(NSInteger)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithUnsignedInteger:(NSUInteger)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(value);
    this
}

- (id)initWithUnsignedLongLong:(u64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedLongLong(value);
    this
}

- (id)initWithShort:(i16)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Short(value);
    this
}

- (id)initWithUnsignedShort:(u16)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedShort(value);
    this
}

- (id)initWithUnsignedLong:(u32)value {
    msg![env; this initWithUnsignedInt:value]
}

- (id)initWithUnsignedChar:(u8)value {
    msg![env; this initWithUnsignedShort:(value as u16)]
}

- (id)initWithChar:(i8)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Char(value);
    this
}

- (bool)boolValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_bool()
}

- (NSInteger)integerValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (i32)intValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (i32)longValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (f32)floatValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_float()
}

- (f64)doubleValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_double()
}

- (i64)longLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_long_long()
}

- (u64)unsignedLongLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_long_long()
}

- (u32)unsignedIntValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (NSUInteger)unsignedIntegerValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (i16)shortValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_short()
}

- (u16)unsignedShortValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_short()
}

- (u32)unsignedLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (u8)unsignedCharValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int() as u8
}

- (i8)charValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_char()
}

- (id)description {
    msg![env; this stringValue]
}

- (id)stringValue {
    msg![env; this descriptionWithLocale:nil]
}
- (id)descriptionWithLocale:(id)locale {
    assert_eq!(locale, nil); // TODO
    // TODO: do not alloc format strings each time
    let format = match env.objc.borrow(this) {
        NSNumberHostObject::Bool(_) | NSNumberHostObject::Char(_) | NSNumberHostObject::Int(_) => from_rust_string(env, "%i".to_string()),
        NSNumberHostObject::Double(_) => from_rust_string(env, "%0.16g".to_string()),
        NSNumberHostObject::Float(_) => from_rust_string(env, "%0.7g".to_string()),
        NSNumberHostObject::LongLong(_) => from_rust_string(env, "%lli".to_string()),
        NSNumberHostObject::Short(_) => from_rust_string(env, "%hi".to_string()),
        NSNumberHostObject::UnsignedInt(_) => from_rust_string(env, "%u".to_string()),
        NSNumberHostObject::UnsignedLongLong(_) => from_rust_string(env, "%llu".to_string()),
        NSNumberHostObject::UnsignedShort(_) => from_rust_string(env, "%hu".to_string()),
    };
    let ns_string_class = env.objc.get_known_class("NSString", &mut env.mem);
    let sel = env.objc.lookup_selector("stringWithFormat:").unwrap();
    // TODO: type info for host-to-host message calls with var-args
    let res = match env.objc.borrow(this) {
        NSNumberHostObject::Bool(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value as i32)),
        NSNumberHostObject::Char(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::Double(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::Float(value) => {
            // Need to promote float to double for the expected argument of %g
            crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value as f64))
        },
        NSNumberHostObject::Int(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::LongLong(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::Short(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::UnsignedInt(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::UnsignedLongLong(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
        NSNumberHostObject::UnsignedShort(value) => crate::objc::msg_send_no_type_checking(env, (ns_string_class, sel, format, *value)),
    };
    release(env, format);
    res
}

- (NSUInteger)hash {
    // The only requirement for [obj hash] is that values that compare equal
    // (via [obj isEqual] have the same hash. Hashing the underlying
    // bits works here.
    let value =
    match env.objc.borrow(this) {
        NSNumberHostObject::Bool(value) => *value as u64,
        NSNumberHostObject::UnsignedLongLong(value) => *value,
        NSNumberHostObject::UnsignedInt(value) => *value as u64,
        NSNumberHostObject::Int(value) => *value as u64,
        NSNumberHostObject::LongLong(value) => *value as u64,
        NSNumberHostObject::Float(value) => value.to_bits() as u64,
        NSNumberHostObject::Double(value) => value.to_bits(),
        NSNumberHostObject::Short(value) => *value as u64,
        NSNumberHostObject::UnsignedShort(value) => *value as u64,
        NSNumberHostObject::Char(value) => *value as u64,
    };
    super::hash_helper(&value)
}

- (bool)isEqual:(id)other {
    if this == other {
        return true;
    }
    let class: Class = msg_class![env; NSNumber class];
    if !msg![env; other isKindOfClass:class] {
        return false;
    }
    msg![env; this isEqualToNumber:other]
}

- (bool)isEqualToNumber:(id)other {
    let res: NSComparisonResult = msg![env; this compare:other];
    res == NSOrderedSame
}

- (NSComparisonResult)compare:(id)other { // NSNumber *
    let num = env.objc.borrow::<NSNumberHostObject>(this);
    let other_num = env.objc.borrow::<NSNumberHostObject>(other);
    let ordering = match (num.is_float(), other_num.is_float()) {
        (false, false) => num.as_i128().cmp(&other_num.as_i128()),
        // In case of having a float, we promote to double for comparison
        _ => {
            // TODO: handle partial cmp fails
            let res = num.as_double().partial_cmp(&other_num.as_double()).unwrap();
            if res == Ordering::Equal {
                // On ties, we compare as i128 as well
                num.as_i128().cmp(&other_num.as_i128())
            } else {
                res
            }
        },
    };
    from_rust_ordering(ordering)
}

// TODO: accessors etc

@end

};

pub fn is_conversion_lossless(env: &mut Environment, this: id, type_: CFNumberType) -> bool {
    let num = env.objc.borrow::<NSNumberHostObject>(this);
    let num2: id = match type_ {
        kCFNumberSInt32Type | kCFNumberIntType => {
            let val: i32 = num.as_int();
            msg_class![env; NSNumber numberWithInt:val]
        }
        kCFNumberFloat32Type | kCFNumberFloatType => {
            let val: f32 = num.as_float();
            msg_class![env; NSNumber numberWithFloat:val]
        }
        kCFNumberSInt16Type | kCFNumberShortType => {
            let val: i16 = num.as_short();
            msg_class![env; NSNumber numberWithShort:val]
        }
        kCFNumberSInt8Type | kCFNumberCharType => {
            let val: i8 = num.as_char();
            msg_class![env; NSNumber numberWithChar:val]
        }
        _ => unimplemented!("is_conversion_lossless for {}", type_),
    };
    msg![env; this isEqualToNumber:num2]
}
