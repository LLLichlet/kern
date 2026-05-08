#![doc = include_str!("../README.md")]

pub use kernc_middle::NodeFacts;
use kernc_ty::DefId;
use kernc_utils::NodeId;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringHints {
    owners: HashMap<DefId, FlowLoweringOwnerHints>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringOwnerHints {
    pub elision: FlowLoweringElisionHints,
    pub forwarding: FlowLoweringForwardingHints,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringElisionHints {
    pub pure_dead_initializer_expr_ids: HashSet<NodeId>,
    pub pure_dead_assignment_expr_ids: HashSet<NodeId>,
    pub elidable_binding_expr_ids: HashSet<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringForwardingHints {
    pub identifier_copy_sources: HashMap<NodeId, String>,
    pub forwardable_binding_sources: HashMap<NodeId, String>,
    pub forwardable_value_expr_ids: HashSet<NodeId>,
}

impl FlowLoweringHints {
    pub fn insert_owner(&mut self, def_id: DefId, hints: FlowLoweringOwnerHints) {
        self.owners.insert(def_id, hints);
    }

    pub fn owner(&self, def_id: DefId) -> Option<&FlowLoweringOwnerHints> {
        self.owners.get(&def_id)
    }
}
