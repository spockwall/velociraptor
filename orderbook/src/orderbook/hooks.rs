use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// Type-erased registry of `Fn(&T)` closures keyed by `TypeId::of::<T>()`.
///
/// Adding a new event kind is zero-change to the registry itself:
/// `registry.on::<NewKind>(|k| ...)` to register, `registry.fire(&kind)` to dispatch.
/// Handlers fire synchronously in registration order.
#[derive(Default)]
pub struct HookRegistry {
    map: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
}

type Hook<T> = Arc<dyn Fn(&T) + Send + Sync + 'static>;

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler for event type `T`.
    pub fn on<T, F>(&mut self, cb: F)
    where
        T: 'static,
        F: Fn(&T) + Send + Sync + 'static,
    {
        let hook: Hook<T> = Arc::new(cb);
        self.map
            .entry(TypeId::of::<T>())
            .or_default()
            .push(Box::new(hook));
    }

    /// Fire all handlers registered for `T`'s type. No-op if none registered.
    pub fn fire<T: 'static>(&self, ev: &T) {
        if let Some(hooks) = self.map.get(&TypeId::of::<T>()) {
            for h in hooks {
                if let Some(f) = h.downcast_ref::<Hook<T>>() {
                    f(ev);
                }
            }
        }
    }
}
