#![cfg(test)]

use super::*;

/// Every variant round-trips through serde under its wire name, the
/// wire name is the `as_wire` form, and every variant carries a doc
/// line; `ALL` is the whole vocabulary (a new variant must join it, or
/// `describe` refuses to compile and this test refuses to pass).
#[test]
fn anchor_state_round_trips_and_is_documented() {
    let mut seen = std::collections::BTreeSet::new();
    for state in AnchorState::ALL {
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, format!("\"{}\"", state.as_wire()));
        let back: AnchorState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        assert!(!state.describe().is_empty(), "{state:?} has a doc line");
        assert!(seen.insert(state.as_wire()), "wire names are distinct");
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        vec!["drifted", "orphaned", "recheck", "resolves"]
    );
    let help = AnchorState::vocabulary_help();
    for state in AnchorState::ALL {
        assert!(help.contains(state.as_wire()) && help.contains(state.describe()));
    }
    // An unknown wire name is refused, never mapped to a neighbour.
    assert!(serde_json::from_str::<AnchorState>("\"resolved\"").is_err());
}

/// A figure without a population cannot be constructed, deserialized or
/// therefore formatted; with one, it prints the count and the statement
/// in one sentence and its JSON carries all three fields.
#[test]
fn figure_refuses_to_exist_without_its_population() {
    assert_eq!(
        AnchorResolutionFigure::new(3, "", true),
        Err(FigureWithoutPopulation)
    );
    assert_eq!(
        AnchorResolutionFigure::new(3, "   ", true),
        Err(FigureWithoutPopulation)
    );
    assert!(
        serde_json::from_str::<AnchorResolutionFigure>(
            r#"{"resolves":3,"population":"","fully_adjudicated":true}"#
        )
        .is_err()
    );
    assert!(serde_json::from_str::<AnchorResolutionFigure>(r#"{"resolves":3}"#).is_err());

    let fig = AnchorResolutionFigure::new(3, "over 4 counted row(s): 4 adjudicated, 0 not", true)
        .unwrap();
    assert_eq!(
        fig.to_string(),
        "3 over 4 counted row(s): 4 adjudicated, 0 not"
    );
    assert_eq!(
        fig.ratio(4),
        "3/4 (75.0%) over 4 counted row(s): 4 adjudicated, 0 not"
    );
    assert_eq!(
        fig.ratio(0),
        "3/0 (n/a) over 4 counted row(s): 4 adjudicated, 0 not"
    );
    let json = serde_json::to_value(&fig).unwrap();
    assert_eq!(json["resolves"], 3);
    assert_eq!(
        json["population"],
        "over 4 counted row(s): 4 adjudicated, 0 not"
    );
    assert_eq!(json["fully_adjudicated"], true);
    assert_eq!(AnchorResolutionFigure::from_json(&json), Some(fig.clone()));
    assert_eq!(
        AnchorResolutionFigure::from_json(&serde_json::json!({"resolves": 3})),
        None
    );
    assert_eq!(fig.count_for_assertions(), 3);
    assert!(!AnchorResolutionFigure::default().fully_adjudicated());
}
