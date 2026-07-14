// KG: SPAN_333_L8_Plugin, plan-333-three-way-hybrid-2026-04-17
// Plugin registry. Thin trait + type-keyed registry (Strategy + Registry pattern).

#![forbid(unsafe_code)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::RwLock;

pub trait Plugin: Any + Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn init(&self) -> Result<(), String> {
        Ok(())
    }
    fn as_any(&self) -> &dyn Any;
}

pub struct PluginRegistry {
    by_type: RwLock<HashMap<TypeId, Box<dyn Plugin>>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self { by_type: RwLock::new(HashMap::new()) }
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P: Plugin + 'static>(&self, plugin: P) -> Result<(), String> {
        plugin.init()?;
        let tid = TypeId::of::<P>();
        let mut g = self.by_type.write().map_err(|e| e.to_string())?;
        if g.contains_key(&tid) {
            return Err(format!("plugin type {} already registered", std::any::type_name::<P>()));
        }
        g.insert(tid, Box::new(plugin));
        Ok(())
    }

    pub fn with<P: Plugin + 'static, R>(&self, f: impl FnOnce(&P) -> R) -> Option<R> {
        let g = self.by_type.read().ok()?;
        let tid = TypeId::of::<P>();
        let boxed = g.get(&tid)?;
        let plugin = boxed.as_any().downcast_ref::<P>()?;
        Some(f(plugin))
    }

    pub fn len(&self) -> usize {
        self.by_type.read().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Foo {
        tag: String,
    }
    impl Plugin for Foo {
        fn name(&self) -> &str { "foo" }
        fn version(&self) -> &str { "0.1" }
        fn as_any(&self) -> &dyn Any { self }
    }

    struct Bar;
    impl Plugin for Bar {
        fn name(&self) -> &str { "bar" }
        fn version(&self) -> &str { "0.2" }
        fn as_any(&self) -> &dyn Any { self }
    }

    #[test]
    fn register_and_invoke() {
        let r = PluginRegistry::new();
        r.register(Foo { tag: "hi".into() }).unwrap();
        let tag = r.with::<Foo, _>(|f| f.tag.clone()).unwrap();
        assert_eq!(tag, "hi");
    }

    #[test]
    fn duplicate_type_rejected() {
        let r = PluginRegistry::new();
        r.register(Foo { tag: "a".into() }).unwrap();
        assert!(r.register(Foo { tag: "b".into() }).is_err());
    }

    #[test]
    fn distinct_types_coexist() {
        let r = PluginRegistry::new();
        r.register(Foo { tag: "x".into() }).unwrap();
        r.register(Bar).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r.with::<Bar, _>(|b| b.name().to_string()).unwrap(), "bar");
    }

    #[test]
    fn unregistered_lookup_returns_none() {
        let r = PluginRegistry::new();
        assert!(r.with::<Foo, _>(|_| 1).is_none());
    }

    #[test]
    fn init_failure_rejects_registration() {
        struct Bad;
        impl Plugin for Bad {
            fn name(&self) -> &str { "bad" }
            fn version(&self) -> &str { "0" }
            fn init(&self) -> Result<(), String> { Err("init fail".into()) }
            fn as_any(&self) -> &dyn Any { self }
        }
        let r = PluginRegistry::new();
        assert!(r.register(Bad).is_err());
        assert!(r.is_empty());
    }
}
