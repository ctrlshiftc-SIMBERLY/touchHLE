/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSSortDescriptor`.

use super::NSComparisonResult;
use crate::objc::{
    autorelease, id, msg, msg_send, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use crate::Environment;

pub struct NSSortDescriptorHostObject {
    key: id,
    ascending: bool,
    selector: Option<SEL>,
}
impl HostObject for NSSortDescriptorHostObject {}

/// Compares two objects using a list of sort descriptors.
pub fn compare_with_descriptors(
    env: &mut Environment,
    descriptors: &[id],
    a: id,
    b: id,
) -> NSComparisonResult {
    for &d in descriptors {
        let res: NSComparisonResult = msg![env; d compareObject:a toObject:b];
        if res != 0 {
            return res;
        }
    }
    0
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSSortDescriptor: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSSortDescriptorHostObject { key: nil, ascending: true, selector: None });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)sortDescriptorWithKey:(id)key ascending:(bool)ascending {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithKey:key ascending:ascending];
    autorelease(env, new)
}

+ (id)sortDescriptorWithKey:(id)key ascending:(bool)ascending selector:(SEL)selector {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithKey:key ascending:ascending selector:selector];
    autorelease(env, new)
}

- (id)initWithKey:(id)key ascending:(bool)ascending {
    let key: id = msg![env; key copy];
    let host = env.objc.borrow_mut::<NSSortDescriptorHostObject>(this);
    host.key = key;
    host.ascending = ascending;
    this
}

- (id)initWithKey:(id)key ascending:(bool)ascending selector:(SEL)selector {
    let this: id = msg![env; this initWithKey:key ascending:ascending];
    env.objc.borrow_mut::<NSSortDescriptorHostObject>(this).selector = Some(selector);
    this
}

- (())dealloc {
    let key = env.objc.borrow::<NSSortDescriptorHostObject>(this).key;
    release(env, key);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)key {
    env.objc.borrow::<NSSortDescriptorHostObject>(this).key
}

- (bool)ascending {
    env.objc.borrow::<NSSortDescriptorHostObject>(this).ascending
}

- (id)reversedSortDescriptor {
    let &NSSortDescriptorHostObject { key, ascending, selector } = env.objc.borrow(this);
    let class: id = msg![env; this class];
    let new: id = msg![env; class alloc];
    let new: id = msg![env; new initWithKey:key ascending:(!ascending)];
    env.objc.borrow_mut::<NSSortDescriptorHostObject>(new).selector = selector;
    autorelease(env, new)
}

- (NSComparisonResult)compareObject:(id)a toObject:(id)b {
    let &NSSortDescriptorHostObject { key, ascending, selector } = env.objc.borrow(this);
    let (va, vb): (id, id) = if key == nil {
        (a, b)
    } else {
        (msg![env; a valueForKey:key], msg![env; b valueForKey:key])
    };
    let res: NSComparisonResult = if va == nil && vb == nil {
        0
    } else if va == nil {
        -1
    } else if vb == nil {
        1
    } else {
        let sel = selector.unwrap_or_else(|| env.objc.lookup_selector("compare:").unwrap());
        msg_send(env, (va, sel, vb))
    };
    if ascending { res } else { -res }
}

- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

@end

};
