/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGestureRecognizer` and common subclasses (iOS 3.2+).
//!
//! Simplified single-touch implementation: tap, pan, long press and swipe are
//! recognized from the first touch on a view. Pinch is never recognized.

use crate::frameworks::core_graphics::{CGFloat, CGPoint};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    id, msg, msg_send, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr, SEL,
};
use crate::Environment;
use std::collections::HashMap;
use std::time::Instant;

pub const UIGestureRecognizerStatePossible: NSInteger = 0;
pub const UIGestureRecognizerStateBegan: NSInteger = 1;
pub const UIGestureRecognizerStateChanged: NSInteger = 2;
pub const UIGestureRecognizerStateEnded: NSInteger = 3;
#[allow(dead_code)]
pub const UIGestureRecognizerStateCancelled: NSInteger = 4;
pub const UIGestureRecognizerStateFailed: NSInteger = 5;

const MOVE_THRESHOLD: CGFloat = 10.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Generic,
    Tap,
    Pan,
    Pinch,
    LongPress,
    Swipe,
}

pub struct GestureHostObject {
    kind: Kind,
    target: id,
    action: Option<SEL>,
    view: id,
    state: NSInteger,
    enabled: bool,
    cancels_touches: bool,
    /// The window the current touch is in (for coordinate conversion).
    window: id,
    start: CGPoint,
    current: CGPoint,
    previous: CGPoint,
    start_time: Option<Instant>,
    last_move_time: Option<Instant>,
    translation_offset: CGPoint,
    velocity: CGPoint,
    tracking: bool,
    direction: u32,
    minimum_press_duration: f64,
}
impl HostObject for GestureHostObject {}

impl GestureHostObject {
    fn new(kind: Kind) -> Self {
        GestureHostObject {
            kind,
            target: nil,
            action: None,
            view: nil,
            state: UIGestureRecognizerStatePossible,
            enabled: true,
            cancels_touches: true,
            window: nil,
            start: CGPoint { x: 0.0, y: 0.0 },
            current: CGPoint { x: 0.0, y: 0.0 },
            previous: CGPoint { x: 0.0, y: 0.0 },
            start_time: None,
            last_move_time: None,
            translation_offset: CGPoint { x: 0.0, y: 0.0 },
            velocity: CGPoint { x: 0.0, y: 0.0 },
            tracking: false,
            direction: 1, // UISwipeGestureRecognizerDirectionRight
            minimum_press_duration: 0.5,
        }
    }
}

#[derive(Default)]
pub struct State {
    /// View -> its gesture recognizers (retained).
    recognizers: HashMap<id, Vec<id>>,
}

fn state(env: &mut Environment) -> &mut State {
    &mut env.framework_state.uikit.ui_gesture_recognizer
}

fn fire(env: &mut Environment, gr: id) {
    let (target, action) = {
        let h = env.objc.borrow::<GestureHostObject>(gr);
        (h.target, h.action)
    };
    if let (true, Some(action)) = (target != nil, action) {
        if std::env::var_os("TOUCHHLE_GESTURE_DEBUG").is_some() {
            let (cur, view, st, kind) = {
                let h = env.objc.borrow::<GestureHostObject>(gr);
                (h.current, h.view, h.state, h.kind)
            };
            let in_view = location_in(env, gr, cur, view);
            let bounds: crate::frameworks::core_graphics::CGRect = msg![env; view bounds];
            log!(
                "GESTURE {:?} state {} window-point {:?} view-point {:?} view-bounds {:?}",
                kind,
                st,
                cur,
                in_view,
                bounds
            );
        }
        () = msg_send(env, (target, action, gr));
    }
}

fn set_state_and_fire(env: &mut Environment, gr: id, new_state: NSInteger) {
    env.objc.borrow_mut::<GestureHostObject>(gr).state = new_state;
    fire(env, gr);
}

fn distance(a: CGPoint, b: CGPoint) -> CGFloat {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Collects the gesture recognizers attached to `view` and its ancestors.
fn recognizers_for(env: &mut Environment, view: id) -> Vec<id> {
    let mut result = Vec::new();
    let mut v = view;
    let mut depth = 0;
    while v != nil && depth < 64 {
        if let Some(list) = state(env).recognizers.get(&v) {
            result.extend(list.iter().copied());
        }
        v = msg![env; v superview];
        depth += 1;
    }
    result
}

/// Location of the first touch in `touches`, in window coordinates.
fn first_touch(env: &mut Environment, touches: id) -> Option<(CGPoint, id)> {
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return None;
    }
    let window: id = msg![env; touch window];
    let p: CGPoint = msg![env; touch locationInView:window];
    Some((p, window))
}

pub fn touches_began(env: &mut Environment, view: id, touches: id) {
    let grs = recognizers_for(env, view);
    if grs.is_empty() {
        return;
    }
    let Some((p, window)) = first_touch(env, touches) else {
        return;
    };
    let now = Instant::now();
    for gr in grs {
        let h = env.objc.borrow_mut::<GestureHostObject>(gr);
        if !h.enabled || h.tracking {
            continue;
        }
        h.tracking = true;
        h.window = window;
        h.start = p;
        h.current = p;
        h.previous = p;
        h.start_time = Some(now);
        h.last_move_time = Some(now);
        h.translation_offset = CGPoint { x: 0.0, y: 0.0 };
        h.velocity = CGPoint { x: 0.0, y: 0.0 };
        h.state = UIGestureRecognizerStatePossible;
    }
}

pub fn touches_moved(env: &mut Environment, view: id, touches: id) {
    let grs = recognizers_for(env, view);
    if grs.is_empty() {
        return;
    }
    let Some((p, _)) = first_touch(env, touches) else {
        return;
    };
    let now = Instant::now();
    for gr in grs {
        let (kind, st, moved, held) = {
            let h = env.objc.borrow_mut::<GestureHostObject>(gr);
            if !h.tracking {
                continue;
            }
            let dt = h
                .last_move_time
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64())
                .max(1.0 / 120.0) as CGFloat;
            h.velocity = CGPoint {
                x: (p.x - h.current.x) / dt,
                y: (p.y - h.current.y) / dt,
            };
            h.previous = h.current;
            h.current = p;
            h.last_move_time = Some(now);
            let held = h
                .start_time
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            (
                h.kind,
                h.state,
                distance(h.start, p),
                held >= h.minimum_press_duration,
            )
        };
        match kind {
            Kind::Tap if moved > MOVE_THRESHOLD => {
                env.objc.borrow_mut::<GestureHostObject>(gr).state = UIGestureRecognizerStateFailed;
            }
            Kind::Pan => {
                if st == UIGestureRecognizerStatePossible && moved > MOVE_THRESHOLD {
                    set_state_and_fire(env, gr, UIGestureRecognizerStateBegan);
                } else if st == UIGestureRecognizerStateBegan
                    || st == UIGestureRecognizerStateChanged
                {
                    set_state_and_fire(env, gr, UIGestureRecognizerStateChanged);
                }
            }
            Kind::LongPress => {
                if st == UIGestureRecognizerStatePossible {
                    if held && moved <= MOVE_THRESHOLD {
                        set_state_and_fire(env, gr, UIGestureRecognizerStateBegan);
                    } else if moved > MOVE_THRESHOLD {
                        env.objc.borrow_mut::<GestureHostObject>(gr).state =
                            UIGestureRecognizerStateFailed;
                    }
                } else if st == UIGestureRecognizerStateBegan
                    || st == UIGestureRecognizerStateChanged
                {
                    set_state_and_fire(env, gr, UIGestureRecognizerStateChanged);
                }
            }
            _ => (),
        }
    }
}

/// Long presses need to begin without the finger moving, so this is polled
/// from the run loop.
pub fn poll(env: &mut Environment) {
    let all: Vec<id> = state(env).recognizers.values().flatten().copied().collect();
    let now = Instant::now();
    for gr in all {
        let ready = {
            let h = env.objc.borrow::<GestureHostObject>(gr);
            h.kind == Kind::LongPress
                && h.tracking
                && h.state == UIGestureRecognizerStatePossible
                && h.start_time.is_some_and(|t| {
                    now.duration_since(t).as_secs_f64() >= h.minimum_press_duration
                })
                && distance(h.start, h.current) <= MOVE_THRESHOLD
        };
        if ready {
            set_state_and_fire(env, gr, UIGestureRecognizerStateBegan);
        }
    }
}

pub fn touches_ended(env: &mut Environment, view: id, touches: id) {
    let grs = recognizers_for(env, view);
    if grs.is_empty() {
        return;
    }
    let Some((p, _)) = first_touch(env, touches) else {
        return;
    };
    let now = Instant::now();
    // If a pan or long press was in progress, a tap must not also fire.
    let mut continuous_active = false;
    for &gr in &grs {
        let h = env.objc.borrow::<GestureHostObject>(gr);
        if h.tracking
            && matches!(h.kind, Kind::Pan | Kind::LongPress)
            && (h.state == UIGestureRecognizerStateBegan
                || h.state == UIGestureRecognizerStateChanged)
        {
            continuous_active = true;
        }
    }
    for gr in grs {
        let (kind, st, moved, elapsed, start, direction) = {
            let h = env.objc.borrow_mut::<GestureHostObject>(gr);
            if !h.tracking {
                continue;
            }
            h.tracking = false;
            h.previous = h.current;
            h.current = p;
            let elapsed = h
                .start_time
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            (
                h.kind,
                h.state,
                distance(h.start, p),
                elapsed,
                h.start,
                h.direction,
            )
        };
        match kind {
            Kind::Tap => {
                if st == UIGestureRecognizerStatePossible
                    && moved <= MOVE_THRESHOLD
                    && elapsed < 0.75
                    && !continuous_active
                {
                    set_state_and_fire(env, gr, UIGestureRecognizerStateEnded);
                }
            }
            Kind::Pan | Kind::LongPress => {
                if st == UIGestureRecognizerStateBegan || st == UIGestureRecognizerStateChanged {
                    set_state_and_fire(env, gr, UIGestureRecognizerStateEnded);
                }
            }
            Kind::Swipe => {
                if moved > 50.0 && elapsed < 0.5 {
                    let dx = p.x - start.x;
                    let dy = p.y - start.y;
                    let dir = if dx.abs() >= dy.abs() {
                        if dx > 0.0 {
                            1
                        } else {
                            2
                        }
                    } else if dy > 0.0 {
                        8
                    } else {
                        4
                    };
                    if direction & dir != 0 {
                        set_state_and_fire(env, gr, UIGestureRecognizerStateEnded);
                    }
                }
            }
            Kind::Pinch | Kind::Generic => (),
        }
        env.objc.borrow_mut::<GestureHostObject>(gr).state = UIGestureRecognizerStatePossible;
    }
}

pub fn add_to_view(env: &mut Environment, view: id, gr: id) {
    retain(env, gr);
    env.objc.borrow_mut::<GestureHostObject>(gr).view = view;
    state(env).recognizers.entry(view).or_default().push(gr);
}

pub fn remove_from_view(env: &mut Environment, view: id, gr: id) {
    let removed = if let Some(list) = state(env).recognizers.get_mut(&view) {
        let before = list.len();
        list.retain(|&g| g != gr);
        before != list.len()
    } else {
        false
    };
    if removed {
        env.objc.borrow_mut::<GestureHostObject>(gr).view = nil;
        release(env, gr);
    }
}

pub fn recognizers_of_view(env: &mut Environment, view: id) -> Vec<id> {
    state(env)
        .recognizers
        .get(&view)
        .cloned()
        .unwrap_or_default()
}

fn location_in(env: &mut Environment, gr: id, point: CGPoint, view: id) -> CGPoint {
    let window = env.objc.borrow::<GestureHostObject>(gr).window;
    if view == nil || window == nil {
        return point;
    }
    let converted: CGPoint = msg![env; view convertPoint:point fromView:window];
    super::ui_touch::fix_unrotated_landscape_point(env, window, view, point, converted)
}

fn alloc_kind(env: &mut Environment, class: id, kind: Kind) -> id {
    let host_object = Box::new(GestureHostObject::new(kind));
    env.objc.alloc_object(class, host_object, &mut env.mem)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIGestureRecognizer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::Generic)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    let h = env.objc.borrow_mut::<GestureHostObject>(this);
    h.target = target; // not retained, as on iOS
    h.action = if action.is_null() { None } else { Some(action) };
    this
}

- (())addTarget:(id)target action:(SEL)action {
    let h = env.objc.borrow_mut::<GestureHostObject>(this);
    h.target = target;
    h.action = Some(action);
}

- (())removeTarget:(id)_target action:(SEL)_action {
    let h = env.objc.borrow_mut::<GestureHostObject>(this);
    h.target = nil;
    h.action = None;
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (NSInteger)state {
    env.objc.borrow::<GestureHostObject>(this).state
}
- (())setState:(NSInteger)new_state {
    env.objc.borrow_mut::<GestureHostObject>(this).state = new_state;
}

- (id)view {
    env.objc.borrow::<GestureHostObject>(this).view
}

- (bool)isEnabled {
    env.objc.borrow::<GestureHostObject>(this).enabled
}
- (())setEnabled:(bool)enabled {
    let h = env.objc.borrow_mut::<GestureHostObject>(this);
    h.enabled = enabled;
    if !enabled {
        h.tracking = false;
        h.state = UIGestureRecognizerStatePossible;
    }
}

- (bool)cancelsTouchesInView {
    env.objc.borrow::<GestureHostObject>(this).cancels_touches
}
- (())setCancelsTouchesInView:(bool)cancels {
    env.objc.borrow_mut::<GestureHostObject>(this).cancels_touches = cancels;
}
- (())setDelaysTouchesBegan:(bool)_delays {}
- (())setDelaysTouchesEnded:(bool)_delays {}
- (())setDelegate:(id)_delegate {}
- (id)delegate { nil }
- (())requireGestureRecognizerToFail:(id)_other {}

- (NSUInteger)numberOfTouches {
    if env.objc.borrow::<GestureHostObject>(this).tracking { 1 } else { 0 }
}

- (CGPoint)locationInView:(id)view {
    let p = env.objc.borrow::<GestureHostObject>(this).current;
    location_in(env, this, p, view)
}

- (CGPoint)locationOfTouch:(NSUInteger)_index inView:(id)view {
    let p = env.objc.borrow::<GestureHostObject>(this).current;
    location_in(env, this, p, view)
}

- (())reset {}

@end

@implementation UITapGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::Tap)
}
- (())setNumberOfTapsRequired:(NSUInteger)_n {}
- (NSUInteger)numberOfTapsRequired { 1 }
- (())setNumberOfTouchesRequired:(NSUInteger)_n {}
- (NSUInteger)numberOfTouchesRequired { 1 }
@end

@implementation UIPanGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::Pan)
}
- (())setMinimumNumberOfTouches:(NSUInteger)_n {}
- (())setMaximumNumberOfTouches:(NSUInteger)_n {}
- (NSUInteger)minimumNumberOfTouches { 1 }
- (NSUInteger)maximumNumberOfTouches { 1 }
- (CGPoint)translationInView:(id)view {
    let h = env.objc.borrow::<GestureHostObject>(this);
    let (start, current, off) = (h.start, h.current, h.translation_offset);
    let a = location_in(env, this, start, view);
    let b = location_in(env, this, current, view);
    CGPoint { x: b.x - a.x + off.x, y: b.y - a.y + off.y }
}
- (())setTranslation:(CGPoint)translation inView:(id)view {
    let h = env.objc.borrow::<GestureHostObject>(this);
    let (start, current) = (h.start, h.current);
    let a = location_in(env, this, start, view);
    let b = location_in(env, this, current, view);
    env.objc.borrow_mut::<GestureHostObject>(this).translation_offset = CGPoint {
        x: translation.x - (b.x - a.x),
        y: translation.y - (b.y - a.y),
    };
}
- (CGPoint)velocityInView:(id)_view {
    env.objc.borrow::<GestureHostObject>(this).velocity
}
@end

@implementation UIPinchGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::Pinch)
}
- (CGFloat)scale { 1.0 }
- (())setScale:(CGFloat)_scale {}
- (CGFloat)velocity { 0.0 }
@end

@implementation UILongPressGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::LongPress)
}
- (())setMinimumPressDuration:(f64)duration {
    env.objc.borrow_mut::<GestureHostObject>(this).minimum_press_duration = duration;
}
- (f64)minimumPressDuration {
    env.objc.borrow::<GestureHostObject>(this).minimum_press_duration
}
- (())setAllowableMovement:(CGFloat)_movement {}
- (())setNumberOfTapsRequired:(NSUInteger)_n {}
- (())setNumberOfTouchesRequired:(NSUInteger)_n {}
@end

@implementation UISwipeGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    alloc_kind(env, this, Kind::Swipe)
}
- (NSUInteger)direction {
    env.objc.borrow::<GestureHostObject>(this).direction
}
- (())setDirection:(NSUInteger)direction {
    env.objc.borrow_mut::<GestureHostObject>(this).direction = direction;
}
- (())setNumberOfTouchesRequired:(NSUInteger)_n {}
@end

};
