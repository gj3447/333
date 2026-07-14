// KG: CONTRACT_333_SchemaRegistry, ATOM_Web_SchemaRegistry
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-sdk-schema-CrdtType/SchemaField/AppSchema/SchemaRegistry/SchemaError
// CRDT schema registration — app declares state fields + CRDT type mapping
// Contract: register_state("blocks", CrdtType::LwwMap) → field available

use std::collections::HashMap;

/// Supported CRDT types for app state fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrdtType {
    LwwRegister,
    LwwMap,
    ORSet,
    RGA,
    GCounter,
    PNCounter,
}

impl std::fmt::Display for CrdtType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::LwwRegister => write!(f, "LWW_Register"),
            Self::LwwMap => write!(f, "LWW_Map"),
            Self::ORSet => write!(f, "OR_Set"),
            Self::RGA => write!(f, "RGA"),
            Self::GCounter => write!(f, "G_Counter"),
            Self::PNCounter => write!(f, "PN_Counter"),
        }
    }
}

/// Schema field definition
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    pub crdt_type: CrdtType,
    pub description: String,
}

/// App schema — registered state fields
#[derive(Debug, Clone)]
pub struct AppSchema {
    pub app_name: String,
    pub fields: HashMap<String, SchemaField>,
}

/// Schema Registry — manages app schemas
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    schemas: HashMap<String, AppSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    AppAlreadyRegistered(String),
    FieldAlreadyExists(String),
    AppNotFound(String),
    FieldNotFound(String),
}

impl SchemaRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register a new app
    pub fn register_app(&mut self, app_name: &str) -> Result<(), SchemaError> {
        if self.schemas.contains_key(app_name) {
            return Err(SchemaError::AppAlreadyRegistered(app_name.into()));
        }
        self.schemas.insert(app_name.into(), AppSchema {
            app_name: app_name.into(),
            fields: HashMap::new(),
        });
        Ok(())
    }

    /// Register a state field for an app
    pub fn register_field(
        &mut self,
        app_name: &str,
        field_name: &str,
        crdt_type: CrdtType,
        description: &str,
    ) -> Result<(), SchemaError> {
        let schema = self.schemas.get_mut(app_name)
            .ok_or_else(|| SchemaError::AppNotFound(app_name.into()))?;

        if schema.fields.contains_key(field_name) {
            return Err(SchemaError::FieldAlreadyExists(field_name.into()));
        }

        schema.fields.insert(field_name.into(), SchemaField {
            name: field_name.into(),
            crdt_type,
            description: description.into(),
        });
        Ok(())
    }

    /// Get schema for an app
    pub fn get_schema(&self, app_name: &str) -> Option<&AppSchema> {
        self.schemas.get(app_name)
    }

    /// Get field type
    pub fn field_type(&self, app_name: &str, field_name: &str) -> Option<CrdtType> {
        self.schemas.get(app_name)?
            .fields.get(field_name)
            .map(|f| f.crdt_type)
    }

    /// List all registered apps
    pub fn apps(&self) -> Vec<&str> {
        self.schemas.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_app_and_fields() {
        let mut reg = SchemaRegistry::new();
        reg.register_app("minecraft").unwrap();
        reg.register_field("minecraft", "blocks", CrdtType::LwwMap, "Block world state").unwrap();
        reg.register_field("minecraft", "inventory", CrdtType::ORSet, "Player inventory").unwrap();

        assert_eq!(reg.field_type("minecraft", "blocks"), Some(CrdtType::LwwMap));
        assert_eq!(reg.field_type("minecraft", "inventory"), Some(CrdtType::ORSet));
    }

    #[test]
    fn duplicate_app_error() {
        let mut reg = SchemaRegistry::new();
        reg.register_app("app1").unwrap();
        assert_eq!(
            reg.register_app("app1"),
            Err(SchemaError::AppAlreadyRegistered("app1".into()))
        );
    }

    #[test]
    fn duplicate_field_error() {
        let mut reg = SchemaRegistry::new();
        reg.register_app("app1").unwrap();
        reg.register_field("app1", "data", CrdtType::LwwMap, "").unwrap();
        assert_eq!(
            reg.register_field("app1", "data", CrdtType::ORSet, ""),
            Err(SchemaError::FieldAlreadyExists("data".into()))
        );
    }

    #[test]
    fn unknown_app_error() {
        let mut reg = SchemaRegistry::new();
        assert_eq!(
            reg.register_field("ghost", "f", CrdtType::LwwMap, ""),
            Err(SchemaError::AppNotFound("ghost".into()))
        );
    }

    #[test]
    fn list_apps() {
        let mut reg = SchemaRegistry::new();
        reg.register_app("minecraft").unwrap();
        reg.register_app("social").unwrap();
        assert_eq!(reg.apps().len(), 2);
    }

    #[test]
    fn minecraft_full_schema() {
        let mut reg = SchemaRegistry::new();
        reg.register_app("minecraft").unwrap();
        reg.register_field("minecraft", "blocks", CrdtType::LwwMap, "Block world").unwrap();
        reg.register_field("minecraft", "chat", CrdtType::RGA, "Chat messages").unwrap();
        reg.register_field("minecraft", "players", CrdtType::ORSet, "Online players").unwrap();

        let schema = reg.get_schema("minecraft").unwrap();
        assert_eq!(schema.fields.len(), 3);
    }
}
