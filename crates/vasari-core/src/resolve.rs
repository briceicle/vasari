use std::collections::HashSet;

use crate::error::VasariError;
use crate::schema::{Action, Attribution, Constraint, Intent, Node, NodeId, Plan, PlanStep};
use crate::store::ObjectStore;

/// The full attribution chain for a file:line query.
///
/// Fields are ordered answer-first: `intents` carries the "why" answer;
/// the remaining fields provide the causal chain that produced it.
///
/// **Line staleness:** line numbers are matched against the ranges recorded
/// at ingest time. If lines have moved since the last `vasari ingest` run
/// (e.g. after a refactor that inserts lines above the queried line),
/// results may be stale. Re-run `vasari ingest` after significant refactors.
#[derive(Debug, Clone)]
pub struct ResolveChain {
    /// The intent(s) that motivated this line of code — the "why" answer.
    /// Sorted by `created_at` descending (most recent amendment first).
    /// Empty only for orphan plans (plans with no `intent_ids`).
    pub intents: Vec<Intent>,
    /// The plan that structured the work.
    pub plan: Plan,
    /// Index into `plan.steps` identifying the specific step that produced this line.
    pub plan_step_index: usize,
    /// The agent action (tool call) that wrote the code.
    pub action: Action,
    /// The attribution node linking the action to this file:line range.
    pub attribution: Attribution,
    /// Constraints that shaped this plan step. Empty until the ingest adapters
    /// populate constraint nodes (v0.1: always empty).
    pub constraints: Vec<Constraint>,
}

impl ResolveChain {
    /// The specific plan step that produced this line.
    pub fn plan_step(&self) -> Option<&PlanStep> {
        self.plan.steps.get(self.plan_step_index)
    }

    /// Attribution confidence: how strongly the ingest adapter believes this
    /// action produced this target. 0.0–1.0; 1.0 = exact range match.
    pub fn confidence(&self) -> f32 {
        self.attribution.confidence
    }

    /// The most recent intent (the originating or most-recently-amended one).
    /// Use this when you need a single "primary" answer for display.
    pub fn primary_intent(&self) -> Option<&Intent> {
        self.intents.first()
    }
}

/// Answer "which intent caused this file:line to exist?"
///
/// Returns the single best-matched [`ResolveChain`] for the given target,
/// or `None` if no attribution covers that line. When multiple attributions
/// overlap the same line, returns the one with the highest confidence.
/// Use [`why_all`] to get all overlapping chains.
///
/// # Errors
/// Returns `Err` if the object store is inaccessible or if a referenced node
/// is missing (which indicates graph corruption — run `vasari fsck` to diagnose).
pub fn why(
    store: &ObjectStore,
    path: &str,
    line: u32,
) -> Result<Option<ResolveChain>, VasariError> {
    let mut chains = why_all(store, path, line)?;
    // Prefer the highest-confidence chain when there are multiple.
    chains.sort_by(|a, b| {
        b.attribution
            .confidence
            .partial_cmp(&a.attribution.confidence)
            .unwrap_or(std::cmp::Ordering::Less) // NaN sorts as lowest confidence
    });
    Ok(chains.into_iter().next())
}

/// Answer "which intents caused this file:line to exist?" for all overlapping attributions.
///
/// Most callers should use [`why`] — this returns the full set only for cases
/// where multiple agent actions have overlapping line ranges (e.g. an edit
/// followed by a cleanup pass).
///
/// # Errors
/// Same as [`why`].
pub fn why_all(
    store: &ObjectStore,
    path: &str,
    line: u32,
) -> Result<Vec<ResolveChain>, VasariError> {
    let attr_ids = store.lookup_attributions(path, line)?;

    // Deduplicate attribution IDs before walking — the append-only index can
    // accumulate duplicates if the store is manually manipulated.
    let mut seen = HashSet::new();
    let attr_ids: Vec<NodeId> = attr_ids.into_iter().filter(|id| seen.insert(id.clone())).collect();

    let mut chains = Vec::new();

    for attr_id in &attr_ids {
        match resolve_one(store, attr_id) {
            Ok(chain) => chains.push(chain),
            Err(VasariError::NodeNotFound(msg)) => {
                // Soft-log integrity gaps: the store may have been partially
                // written. The caller (cmd_why) still gets partial results.
                eprintln!("vasari: warn: skipping attribution {attr_id}: {msg}");
            }
            Err(e) => return Err(e),
        }
    }

    Ok(chains)
}

/// Walk a single attribution ID to build a complete ResolveChain.
fn resolve_one(store: &ObjectStore, attr_id: &NodeId) -> Result<ResolveChain, VasariError> {
    let attr = match store.get(attr_id)? {
        Some(Node::Attribution(a)) => a,
        Some(_) => {
            return Err(VasariError::NodeNotFound(format!(
                "id {attr_id} is not an Attribution node"
            )))
        }
        None => {
            return Err(VasariError::NodeNotFound(format!(
                "attribution node {attr_id} not found — run `vasari fsck` to rebuild the index"
            )))
        }
    };

    let action = match store.get(&attr.action_id)? {
        Some(Node::Action(a)) => a,
        Some(_) => {
            return Err(VasariError::NodeNotFound(format!(
                "action id {} is not an Action node",
                attr.action_id
            )))
        }
        None => {
            return Err(VasariError::NodeNotFound(format!(
                "action node {} not found — graph may be incomplete; run `vasari ingest` again",
                attr.action_id
            )))
        }
    };

    let plan = match store.get(&action.plan_ref.plan_id)? {
        Some(Node::Plan(p)) => p,
        Some(_) => {
            return Err(VasariError::NodeNotFound(format!(
                "plan id {} is not a Plan node",
                action.plan_ref.plan_id
            )))
        }
        None => {
            return Err(VasariError::NodeNotFound(format!(
                "plan node {} not found — graph may be incomplete; run `vasari ingest` again",
                action.plan_ref.plan_id
            )))
        }
    };

    let step_index = action.plan_ref.step_index;
    if step_index >= plan.steps.len() {
        return Err(VasariError::PlanStepOutOfBounds {
            plan_id: plan.id.to_string(),
            step_index,
            step_count: plan.steps.len(),
        });
    }

    // Resolve all intents. Missing individual intents are soft-logged —
    // the plan may legitimately reference a pruned or deferred intent.
    let mut intents: Vec<Intent> = Vec::new();
    for intent_id in &plan.intent_ids {
        match store.get(intent_id)? {
            Some(Node::Intent(i)) => intents.push(i),
            Some(_) => eprintln!(
                "vasari: warn: intent id {intent_id} is not an Intent node — skipping"
            ),
            None => eprintln!(
                "vasari: warn: intent node {intent_id} not found — skipping"
            ),
        }
    }
    // Most recent amendment first.
    intents.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Constraints are empty until ingest adapters populate them (v0.1).
    let constraints: Vec<Constraint> = Vec::new();

    Ok(ResolveChain {
        intents,
        plan,
        plan_step_index: step_index,
        action,
        attribution: attr,
        constraints,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Attribution, AttributionTarget, Node, Plan, PlanRef, PlanStep};
    use chrono::Utc;

    fn make_store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        (store, dir)
    }

    fn make_intent(store: &ObjectStore, source: &str, text: &str) -> Intent {
        let intent = Intent::new(source.into(), text.into(), vec![]);
        store.put(&Node::Intent(intent.clone())).unwrap();
        intent
    }

    fn make_plan(store: &ObjectStore, intent_ids: Vec<NodeId>, steps: Vec<PlanStep>) -> Plan {
        let plan = Plan::new(intent_ids, steps, vec![]);
        store.put(&Node::Plan(plan.clone())).unwrap();
        plan
    }

    fn make_action(
        store: &ObjectStore,
        tool: &str,
        plan_id: NodeId,
        step_index: usize,
    ) -> Action {
        let action = Action::new(
            tool.into(),
            serde_json::json!({ "path": "src/auth.ts" }),
            String::new(),
            Utc::now(),
            PlanRef { plan_id, step_index },
            vec![],
        );
        store.put(&Node::Action(action.clone())).unwrap();
        action
    }

    fn make_attr(
        store: &ObjectStore,
        action_id: NodeId,
        path: &str,
        start: u32,
        end: u32,
        confidence: f32,
    ) -> Attribution {
        let attr = Attribution::new(
            action_id,
            AttributionTarget::LineRange { path: path.into(), start, end },
            confidence,
            vec![],
            vec![],
        );
        store.put(&Node::Attribution(attr.clone())).unwrap();
        attr
    }

    #[test]
    fn why_returns_none_for_empty_store() {
        let (store, _dir) = make_store();
        assert!(why(&store, "src/main.rs", 1).unwrap().is_none());
    }

    #[test]
    fn why_all_returns_empty_for_empty_store() {
        let (store, _dir) = make_store();
        assert!(why_all(&store, "src/main.rs", 1).unwrap().is_empty());
    }

    #[test]
    fn why_full_happy_path() {
        let (store, _dir) = make_store();

        let intent = make_intent(&store, "ACME-411", "Add JWT verification before the user-id lookup");
        let plan = make_plan(
            &store,
            vec![intent.id.clone()],
            vec![
                PlanStep { goal: "Read existing auth code".into(), constraints: vec![] },
                PlanStep {
                    goal: "Add JWT verification middleware".into(),
                    constraints: vec!["no async/await".into()],
                },
            ],
        );
        let action = make_action(&store, "edit_file", plan.id.clone(), 1);
        make_attr(&store, action.id.clone(), "src/auth.ts", 40, 55, 1.0);

        let chain = why(&store, "src/auth.ts", 47).unwrap().expect("chain must be found");

        assert_eq!(chain.intents.len(), 1);
        assert_eq!(chain.intents[0].text, "Add JWT verification before the user-id lookup");
        assert_eq!(chain.intents[0].id, intent.id);
        assert_eq!(chain.plan_step_index, 1);

        let step = chain.plan_step().expect("plan step must be present");
        assert_eq!(step.goal, "Add JWT verification middleware");

        assert!((chain.confidence() - 1.0).abs() < f32::EPSILON);
        assert!(chain.constraints.is_empty());

        let primary = chain.primary_intent().expect("primary intent must be present");
        assert_eq!(primary.id, intent.id);
    }

    #[test]
    fn why_returns_highest_confidence_when_multiple() {
        let (store, _dir) = make_store();

        let i1 = make_intent(&store, "ACME-411", "First intent");
        let p1 = make_plan(&store, vec![i1.id.clone()], vec![PlanStep { goal: "step A".into(), constraints: vec![] }]);
        let a1 = make_action(&store, "edit_file", p1.id.clone(), 0);
        make_attr(&store, a1.id.clone(), "src/auth.ts", 40, 55, 0.9);

        let i2 = make_intent(&store, "ACME-419", "Second intent");
        let p2 = make_plan(&store, vec![i2.id.clone()], vec![PlanStep { goal: "step B".into(), constraints: vec![] }]);
        let a2 = make_action(&store, "str_replace", p2.id.clone(), 0);
        make_attr(&store, a2.id.clone(), "src/auth.ts", 44, 50, 0.7);

        let chains = why_all(&store, "src/auth.ts", 47).unwrap();
        assert_eq!(chains.len(), 2);

        let best = why(&store, "src/auth.ts", 47).unwrap().unwrap();
        assert!((best.confidence() - 0.9).abs() < f32::EPSILON);
        assert_eq!(best.intents[0].source, "ACME-411");
    }

    #[test]
    fn step_index_out_of_bounds_returns_error() {
        let (store, _dir) = make_store();

        let intent = make_intent(&store, "src", "test");
        let plan = make_plan(
            &store,
            vec![intent.id.clone()],
            vec![PlanStep { goal: "only step".into(), constraints: vec![] }],
        );
        // Action points to step_index=5 but plan has only 1 step
        let action = make_action(&store, "edit_file", plan.id.clone(), 5);
        make_attr(&store, action.id.clone(), "src/main.rs", 1, 10, 1.0);

        let result = why_all(&store, "src/main.rs", 5);
        assert!(
            matches!(
                result,
                Err(VasariError::PlanStepOutOfBounds { step_index: 5, step_count: 1, .. })
            ),
            "expected PlanStepOutOfBounds, got: {result:?}"
        );
    }

    #[test]
    fn plan_with_multiple_intents_resolves_all() {
        let (store, _dir) = make_store();

        let i1 = make_intent(&store, "ACME-411", "First");
        let i2 = make_intent(&store, "ACME-412", "Second");
        let plan = make_plan(
            &store,
            vec![i1.id.clone(), i2.id.clone()],
            vec![PlanStep { goal: "do thing".into(), constraints: vec![] }],
        );
        let action = make_action(&store, "edit_file", plan.id.clone(), 0);
        make_attr(&store, action.id.clone(), "src/lib.rs", 1, 5, 1.0);

        let chain = why(&store, "src/lib.rs", 3).unwrap().unwrap();
        assert_eq!(chain.intents.len(), 2);
    }

    #[test]
    fn plan_with_zero_intents_succeeds() {
        let (store, _dir) = make_store();

        let plan = make_plan(&store, vec![], vec![PlanStep { goal: "orphan step".into(), constraints: vec![] }]);
        let action = make_action(&store, "edit_file", plan.id.clone(), 0);
        make_attr(&store, action.id.clone(), "src/empty.rs", 1, 1, 1.0);

        let chain = why(&store, "src/empty.rs", 1).unwrap().unwrap();
        assert!(chain.intents.is_empty());
    }

    #[test]
    fn missing_action_node_is_soft_logged_not_error() {
        let (store, _dir) = make_store();
        // Attribution references an action_id that was never stored
        let ghost_action_id = NodeId("deadbeef".repeat(8));
        let attr = Attribution::new(
            ghost_action_id,
            AttributionTarget::LineRange { path: "src/ghost.rs".into(), start: 1, end: 5 },
            1.0,
            vec![],
            vec![],
        );
        store.put(&Node::Attribution(attr)).unwrap();

        // why_all soft-logs NodeNotFound for missing action, returns empty
        let chains = why_all(&store, "src/ghost.rs", 3).unwrap();
        assert!(chains.is_empty(), "missing action should be soft-logged, not crash");
    }
}
