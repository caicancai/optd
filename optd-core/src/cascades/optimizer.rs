use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    sync::Arc,
};

use anyhow::Result;

use crate::{
    cost::CostModel,
    property::{PropertyBuilder, PropertyBuilderAny},
    rel_node::{RelNodeRef, RelNodeTyp},
    rules::Rule,
};

use super::{
    memo::{GroupInfo, RelMemoNodeRef},
    tasks::OptimizeGroupTask,
    Memo, Task,
};

pub type RuleId = usize;

#[derive(Default, Clone, Debug)]
pub struct OptimizerContext {
    pub upper_bound: Option<f64>,
}

pub struct CascadesOptimizer<T: RelNodeTyp> {
    memo: Memo<T>,
    tasks: VecDeque<Box<dyn Task<T>>>,
    explored_group: HashSet<GroupId>,
    fired_rules: HashMap<ExprId, HashSet<RuleId>>,
    rules: Arc<[Arc<dyn Rule<T>>]>,
    cost: Arc<dyn CostModel<T>>,
    property_builders: Arc<[Box<dyn PropertyBuilderAny<T>>]>,
    pub ctx: OptimizerContext,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct GroupId(pub(super) usize);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct ExprId(pub usize);

impl Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "!{}", self.0)
    }
}

impl Display for ExprId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: RelNodeTyp> CascadesOptimizer<T> {
    pub fn new(
        rules: Vec<Arc<dyn Rule<T>>>,
        cost: Box<dyn CostModel<T>>,
        property_builders: Vec<Box<dyn PropertyBuilderAny<T>>>,
    ) -> Self {
        let tasks = VecDeque::new();
        let property_builders: Arc<[_]> = property_builders.into();
        let memo = Memo::new(property_builders.clone());
        Self {
            memo,
            tasks,
            explored_group: HashSet::new(),
            fired_rules: HashMap::new(),
            rules: rules.into(),
            cost: cost.into(),
            ctx: OptimizerContext::default(),
            property_builders,
        }
    }

    pub fn cost(&self) -> Arc<dyn CostModel<T>> {
        self.cost.clone()
    }

    pub(super) fn rules(&self) -> Arc<[Arc<dyn Rule<T>>]> {
        self.rules.clone()
    }

    pub fn dump(&self) {
        for group_id in self.memo.get_all_group_ids() {
            let winner = if let Some(ref winner) = self.memo.get_group_info(group_id).winner {
                if winner.impossible {
                    "winner=<impossible>".to_string()
                } else {
                    format!(
                        "winner={} cost={} {}",
                        winner.expr_id,
                        self.cost.explain(&winner.cost),
                        self.memo.get_expr_memoed(winner.expr_id)
                    )
                }
            } else {
                "winner=None".to_string()
            };
            println!("group_id={} {}", group_id, winner);
            let group = self.memo.get_group(group_id);
            for (id, property) in self.property_builders.iter().enumerate() {
                println!(
                    "  {}={}",
                    property.property_name(),
                    property.display(group.properties[id].as_ref())
                )
            }
            for expr_id in self.memo.get_all_exprs_in_group(group_id) {
                let memo_node = self.memo.get_expr_memoed(expr_id);
                println!("  expr_id={} | {}", expr_id, memo_node);
                let bindings = self.memo.get_all_expr_bindings(expr_id, false, Some(1));
                for binding in bindings {
                    println!("    {}", binding);
                }
            }
        }
    }

    pub fn optimize(&mut self, root_rel: RelNodeRef<T>) -> Result<RelNodeRef<T>> {
        let (group_id, _) = self.add_group_expr(root_rel, None);
        self.tasks
            .push_back(Box::new(OptimizeGroupTask::new(group_id)));
        // get the task from the stack
        while let Some(task) = self.tasks.pop_back() {
            let new_tasks = task.execute(self)?;
            self.tasks.extend(new_tasks);
        }
        self.memo.get_best_group_binding(group_id)
    }

    pub fn resolve_group_id(&self, root_rel: RelNodeRef<T>) -> GroupId {
        let (group_id, _) = self.get_expr_info(root_rel);
        group_id
    }

    pub(super) fn get_all_exprs_in_group(&self, group_id: GroupId) -> Vec<ExprId> {
        self.memo.get_all_exprs_in_group(group_id)
    }

    pub(super) fn get_expr_info(&self, expr: RelNodeRef<T>) -> (GroupId, ExprId) {
        self.memo.get_expr_info(expr)
    }

    pub(super) fn add_group_expr(
        &mut self,
        expr: RelNodeRef<T>,
        group_id: Option<GroupId>,
    ) -> (GroupId, ExprId) {
        self.memo.add_new_group_expr(expr, group_id)
    }

    pub(super) fn get_group_info(&self, group_id: GroupId) -> GroupInfo {
        self.memo.get_group_info(group_id)
    }

    pub(super) fn update_group_info(&mut self, group_id: GroupId, group_info: GroupInfo) {
        self.memo.update_group_info(group_id, group_info)
    }

    pub fn get_property<P: PropertyBuilder<T>>(&self, group_id: GroupId, idx: usize) -> P::Prop {
        self.memo.get_group(group_id).properties[idx]
            .downcast_ref::<P::Prop>()
            .unwrap()
            .clone()
    }

    pub(super) fn get_group_id(&self, expr_id: ExprId) -> GroupId {
        self.memo.get_group_id(expr_id)
    }

    pub(super) fn get_expr_memoed(&self, expr_id: ExprId) -> RelMemoNodeRef<T> {
        self.memo.get_expr_memoed(expr_id)
    }

    pub(super) fn get_all_expr_bindings(
        &self,
        expr_id: ExprId,
        level: Option<usize>,
    ) -> Vec<RelNodeRef<T>> {
        self.memo.get_all_expr_bindings(expr_id, false, level)
    }

    pub(super) fn get_all_expr_physical_bindings(
        &self,
        expr_id: ExprId,
        level: Option<usize>,
    ) -> Vec<RelNodeRef<T>> {
        self.memo.get_all_expr_bindings(expr_id, true, level)
    }

    pub(super) fn is_group_explored(&self, group_id: GroupId) -> bool {
        self.explored_group.contains(&group_id)
    }

    pub(super) fn mark_group_explored(&mut self, group_id: GroupId) {
        self.explored_group.insert(group_id);
    }

    pub(super) fn is_rule_fired(&self, group_expr_id: ExprId, rule_id: RuleId) -> bool {
        self.fired_rules
            .get(&group_expr_id)
            .map(|rules| rules.contains(&rule_id))
            .unwrap_or(false)
    }

    pub(super) fn mark_rule_fired(&mut self, group_expr_id: ExprId, rule_id: RuleId) {
        self.fired_rules
            .entry(group_expr_id)
            .or_default()
            .insert(rule_id);
    }
}
