/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSPredicate` (simple subset).
//!
//! Supports formats of the form `keyPath OP value`, where OP is one of
//! `==`, `=`, `!=`, `<>`, `<`, `<=`, `>`, `>=` or `LIKE`, and value is a
//! literal (`true`, `false`, `YES`, `NO`, `nil`, a number or a quoted string)
//! or a format argument (`%@`, `%d`, `%i`, `%u`, `%f`). Compound predicates
//! joined with `AND`/`&&` are supported. Block predicates are supported.

use super::ns_string::{from_rust_string, to_rust_string};
use super::NSUInteger;
use crate::abi::{CallFromHost, GuestFunction, VaList};
use crate::frameworks::foundation::NSComparisonResult;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
}

#[derive(Debug)]
enum Value {
    Object(id), // retained
    Number(f64),
    Nil,
}

#[derive(Debug)]
struct Comparison {
    key: String,
    op: Op,
    value: Value,
}

enum Predicate {
    Comparisons(Vec<Comparison>),
    Block(id), // retained block
    Constant(bool),
}

pub struct NSPredicateHostObject {
    predicate: Predicate,
}
impl HostObject for NSPredicateHostObject {}

fn parse_value(env: &mut Environment, token: &str, args: &mut VaList) -> Value {
    match token {
        "%@" | "%K" => {
            let obj: id = args.next(env);
            if obj == nil {
                Value::Nil
            } else {
                retain(env, obj);
                Value::Object(obj)
            }
        }
        "%d" | "%i" | "%ld" | "%li" => {
            let v: i32 = args.next(env);
            Value::Number(v as f64)
        }
        "%u" | "%lu" => {
            let v: u32 = args.next(env);
            Value::Number(v as f64)
        }
        "%f" | "%lf" => {
            let v: f64 = args.next(env);
            Value::Number(v)
        }
        "true" | "TRUE" | "YES" | "yes" => Value::Number(1.0),
        "false" | "FALSE" | "NO" | "no" => Value::Number(0.0),
        "nil" | "NIL" | "NULL" | "null" => Value::Nil,
        t if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
            || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2) =>
        {
            let s = from_rust_string(env, t[1..t.len() - 1].to_string());
            Value::Object(s)
        }
        t => match t.parse::<f64>() {
            Ok(n) => Value::Number(n),
            Err(_) => {
                log!("TODO: NSPredicate value {:?}, treating as nil", t);
                Value::Nil
            }
        },
    }
}

fn parse(env: &mut Environment, format: &str, mut args: VaList) -> Predicate {
    let format = format.trim();
    if format.eq_ignore_ascii_case("TRUEPREDICATE") {
        return Predicate::Constant(true);
    }
    if format.eq_ignore_ascii_case("FALSEPREDICATE") {
        return Predicate::Constant(false);
    }
    let mut comparisons = Vec::new();
    let normalized = format.replace("&&", " AND ");
    for part in normalized.split(" AND ") {
        let tokens: Vec<&str> = part.split_whitespace().collect();
        if tokens.len() != 3 {
            log!("TODO: unsupported NSPredicate format {:?}", format);
            continue;
        }
        let op = match tokens[1].to_ascii_uppercase().as_str() {
            "==" | "=" => Op::Eq,
            "!=" | "<>" => Op::Ne,
            "<" => Op::Lt,
            "<=" | "=<" => Op::Le,
            ">" => Op::Gt,
            ">=" | "=>" => Op::Ge,
            "LIKE" | "LIKE[C]" | "LIKE[CD]" => Op::Like,
            other => {
                log!("TODO: unsupported NSPredicate operator {:?}", other);
                Op::Eq
            }
        };
        let key = if tokens[0] == "%K" {
            let k: id = args.next(env);
            to_rust_string(env, k).to_string()
        } else {
            tokens[0].to_string()
        };
        let value = parse_value(env, tokens[2], &mut args);
        comparisons.push(Comparison { key, op, value });
    }
    Predicate::Comparisons(comparisons)
}

fn like(pattern: &[char], s: &[char]) -> bool {
    match pattern.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| like(&pattern[1..], &s[i..])),
        Some('?') => !s.is_empty() && like(&pattern[1..], &s[1..]),
        Some(&c) => !s.is_empty() && s[0] == c && like(&pattern[1..], &s[1..]),
    }
}

fn number_of(env: &mut Environment, obj: id) -> Option<f64> {
    if obj == nil {
        return None;
    }
    let number_class = env.objc.get_known_class("NSNumber", &mut env.mem);
    let class: id = msg![env; obj class];
    if env.objc.class_is_subclass_of(class, number_class) {
        let v: f64 = msg![env; obj doubleValue];
        Some(v)
    } else {
        None
    }
}

fn evaluate_comparison(env: &mut Environment, c: &Comparison, object: id) -> bool {
    let key = if c.key == "SELF" {
        None
    } else {
        Some(from_rust_string(env, c.key.clone()))
    };
    let lhs: id = match key {
        None => object,
        Some(k) => {
            let v: id = msg![env; object valueForKey:k];
            release(env, k);
            v
        }
    };
    match (&c.value, c.op) {
        (Value::Nil, Op::Eq) => lhs == nil,
        (Value::Nil, Op::Ne) => lhs != nil,
        (Value::Nil, _) => false,
        (Value::Number(n), op) => {
            let Some(l) = number_of(env, lhs) else {
                return op == Op::Ne;
            };
            match op {
                Op::Eq | Op::Like => l == *n,
                Op::Ne => l != *n,
                Op::Lt => l < *n,
                Op::Le => l <= *n,
                Op::Gt => l > *n,
                Op::Ge => l >= *n,
            }
        }
        (Value::Object(o), op) => {
            let o = *o;
            if lhs == nil {
                return op == Op::Ne;
            }
            if let (Some(l), Some(r)) = (number_of(env, lhs), number_of(env, o)) {
                return match op {
                    Op::Eq | Op::Like => l == r,
                    Op::Ne => l != r,
                    Op::Lt => l < r,
                    Op::Le => l <= r,
                    Op::Gt => l > r,
                    Op::Ge => l >= r,
                };
            }
            match op {
                Op::Eq => msg![env; lhs isEqual:o],
                Op::Ne => {
                    let eq: bool = msg![env; lhs isEqual:o];
                    !eq
                }
                Op::Like => {
                    let pat: Vec<char> = to_rust_string(env, o).chars().collect();
                    let s: Vec<char> = to_rust_string(env, lhs).chars().collect();
                    like(&pat, &s)
                }
                _ => {
                    let res: NSComparisonResult = msg![env; lhs compare:o];
                    match op {
                        Op::Lt => res < 0,
                        Op::Le => res <= 0,
                        Op::Gt => res > 0,
                        Op::Ge => res >= 0,
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}

pub fn evaluate(env: &mut Environment, predicate: id, object: id) -> bool {
    let host = env.objc.borrow::<NSPredicateHostObject>(predicate);
    match &host.predicate {
        Predicate::Constant(b) => *b,
        Predicate::Block(block) => {
            let block = *block;
            let invoke: u32 = env.mem.read(block.cast::<u32>() + 3);
            let invoke = GuestFunction::from_addr_with_thumb_bit(invoke);
            let res: u32 = invoke.call_from_host(env, (block, object, nil));
            (res & 0xff) != 0
        }
        Predicate::Comparisons(cs) => {
            // Temporarily take the comparisons out to avoid borrowing issues.
            let cs: Vec<Comparison> = cs
                .iter()
                .map(|c| Comparison {
                    key: c.key.clone(),
                    op: c.op,
                    value: match &c.value {
                        Value::Object(o) => Value::Object(*o),
                        Value::Number(n) => Value::Number(*n),
                        Value::Nil => Value::Nil,
                    },
                })
                .collect();
            cs.iter().all(|c| evaluate_comparison(env, c, object))
        }
    }
}

pub fn filter(env: &mut Environment, array: id, predicate: id) -> Vec<id> {
    let count: NSUInteger = msg![env; array count];
    let mut result = Vec::new();
    for i in 0..count {
        let obj: id = msg![env; array objectAtIndex:i];
        if evaluate(env, predicate, obj) {
            result.push(obj);
        }
    }
    result
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSPredicate: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSPredicateHostObject { predicate: Predicate::Constant(true) });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)predicateWithFormat:(id)format, ...args {
    let format_str = to_rust_string(env, format).to_string();
    let predicate = parse(env, &format_str, args.start());
    log_dbg!("NSPredicate {:?}", format_str);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<NSPredicateHostObject>(new).predicate = predicate;
    autorelease(env, new)
}

+ (id)predicateWithValue:(bool)value {
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<NSPredicateHostObject>(new).predicate = Predicate::Constant(value);
    autorelease(env, new)
}

+ (id)predicateWithBlock:(id)block {
    let block: id = crate::objc::blocks::block_copy(env, block.cast_const().cast()).cast();
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<NSPredicateHostObject>(new).predicate = Predicate::Block(block);
    autorelease(env, new)
}

- (bool)evaluateWithObject:(id)object {
    evaluate(env, this, object)
}

- (())dealloc {
    let objs: Vec<id> = match &env.objc.borrow::<NSPredicateHostObject>(this).predicate {
        Predicate::Comparisons(cs) => cs
            .iter()
            .filter_map(|c| if let Value::Object(o) = c.value { Some(o) } else { None })
            .collect(),
        _ => Vec::new(),
    };
    for o in objs {
        release(env, o);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

@end

};
