//! Entity taxonomy and edge kinds shared between search and analyzer.
//!
//! Mirrors `trusty_search_core::entity::{EntityType, EdgeKind, RawEntity}`
//! with variant names preserved so JSON round-trips.

use serde::{Deserialize, Serialize};

/// Taxonomy of program entities surfaced from the AST. Variant names match
/// `trusty_search_core::entity::EntityType` so wire-format payloads decode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    NamedType,
    TraitBound,
    ModulePath,
    ErrorVariant,
    TestRelation,
    DocConcept,
    Annotation,
    LiteralString,
    TypeAlias,
    ConstantSymbol,
    ExternalCrate,
    ConceptCluster,
    NaturalLanguagePhrase,
}

/// Edge kinds for the symbol knowledge graph. Mirrors the search side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    CallsFunction,
    CalledByFunction,
    Implements,
    UsesType,
    Derives,
    ModuleContains,
    ReExports,
    RaisesError,
    Configures,
    TestedBy,
    TestUsesFixture,
    CoOccursInTest,
    Documents,
    ReferencesConcept,
    Aliases,
    ErrorDescribes,
}

/// One extracted entity, anchored to a file + line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEntity {
    pub id: String,
    pub entity_type: EntityType,
    pub text: String,
    #[serde(default)]
    pub span: (usize, usize),
    pub file: String,
    pub line: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_type_round_trips() {
        let kinds = [
            EntityType::NamedType,
            EntityType::ModulePath,
            EntityType::TestRelation,
            EntityType::ConceptCluster,
        ];
        for k in kinds {
            let s = serde_json::to_string(&k).unwrap();
            let back: EntityType = serde_json::from_str(&s).unwrap();
            assert_eq!(k, back);
        }
    }
}
