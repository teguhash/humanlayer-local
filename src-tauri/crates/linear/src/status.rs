//! Board stage -> Linear workflow state. Moves only go forward, so a task
//! that loops back (say from review to implement) never drags its ticket
//! back. Ported from upstream `linear-status-progression.ts` (ranking) and
//! the RPI skill -> status names in `sessions/events/create.ts`.

use crate::State;

/// Upstream's category order; canceled is terminal.
fn type_rank(kind: &str) -> u8 {
    match kind {
        "triage" => 0,
        "backlog" => 1,
        "unstarted" => 2,
        "started" => 3,
        "completed" => 4,
        "canceled" => 5,
        _ => 3,
    }
}

/// True when `target` is strictly further along than `current`: category
/// first, then board position within a category.
pub fn is_more_progressed(target: &State, current: &State) -> bool {
    let (t, c) = (type_rank(&target.kind), type_rank(&current.kind));
    if t != c {
        return t > c;
    }
    target.position > current.position
}

/// Preferred state names per stage: upstream's RPI columns first (created by
/// its `ensure-columns`), then Linear's default names.
fn names(stage: &str) -> &'static [&'static str] {
    match stage {
        "research" => &["research-in-progress"],
        "plan" => &["plan-in-progress"],
        "implement" => &["in-dev", "In Progress"],
        "review" => &["code-review", "In Review"],
        "done" => &["Done"],
        _ => &[],
    }
}

/// The state category to fall back to when no name matches.
fn fallback_kind(stage: &str) -> Option<&'static str> {
    match stage {
        "research" | "plan" | "implement" | "review" => Some("started"),
        "done" => Some("completed"),
        _ => None,
    }
}

/// The state `stage` maps to in this team, if any. Backlog maps to none.
pub fn state_for_stage<'a>(stage: &str, states: &'a [State]) -> Option<&'a State> {
    for n in names(stage) {
        if let Some(s) = states.iter().find(|s| s.name.eq_ignore_ascii_case(n)) {
            return Some(s);
        }
    }
    let kind = fallback_kind(stage)?;
    states
        .iter()
        .filter(|s| s.kind == kind)
        .min_by(|a, b| a.position.total_cmp(&b.position))
}

/// The state to move a ticket to when its task enters `stage`, or None if
/// that would not move it forward.
pub fn advance<'a>(stage: &str, current: &State, states: &'a [State]) -> Option<&'a State> {
    state_for_stage(stage, states).filter(|t| is_more_progressed(t, current))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(name: &str, kind: &str, pos: f64) -> State {
        State {
            id: name.into(),
            name: name.into(),
            kind: kind.into(),
            position: pos,
        }
    }

    #[test]
    fn default_team_workflow() {
        let states = vec![
            st("Backlog", "backlog", 0.0),
            st("Todo", "unstarted", 1.0),
            st("In Progress", "started", 2.0),
            st("In Review", "started", 3.0),
            st("Done", "completed", 4.0),
            st("Canceled", "canceled", 5.0),
        ];
        let todo = &states[1];
        let name = |stage, cur: &State| advance(stage, cur, &states).map(|s| s.name.clone());
        assert_eq!(name("backlog", todo), None);
        assert_eq!(name("research", todo).as_deref(), Some("In Progress"));
        assert_eq!(name("review", todo).as_deref(), Some("In Review"));
        // Never backwards: from In Review, implement is a no-op.
        assert_eq!(name("implement", &states[3]), None);
        assert_eq!(name("done", &states[3]).as_deref(), Some("Done"));
        // Canceled is terminal.
        assert_eq!(name("done", &states[5]), None);
    }

    #[test]
    fn rpi_columns_win_over_fallback() {
        let states = vec![
            st("Todo", "unstarted", 1.0),
            st("research-in-progress", "started", 3.0),
            st("plan-in-progress", "started", 5.0),
            st("in-dev", "started", 7.0),
            st("In Progress", "started", 2.0),
        ];
        let s = |stage| state_for_stage(stage, &states).map(|s| s.name.as_str());
        assert_eq!(s("research"), Some("research-in-progress"));
        assert_eq!(s("plan"), Some("plan-in-progress"));
        assert_eq!(s("implement"), Some("in-dev"));
        // No code-review / In Review: the first started state.
        assert_eq!(s("review"), Some("In Progress"));
        assert_eq!(s("done"), None);
    }
}
