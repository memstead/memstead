//! The MCP half of the coverage gate: every tool either declares the
//! axes its clean-reading output examined or declares why it emits no
//! verdict. Discovery walks the server's live tool router, never a
//! hand-kept list, so a tool cannot land outside the registry's
//! sight; the vocabulary comes from `memstead_base::ops::coverage`,
//! so a new axis fails every verdict row that has not met it. The
//! rule and its rationale: `memstead_base::ops::coverage`.

use memstead_base::ops::coverage::{validate_coverage, verdict_axes};

fn assert_covered(
    names: Vec<String>,
    registry: Vec<memstead_base::ops::coverage::SurfaceCoverage>,
) {
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let vocab = verdict_axes();
    let findings = validate_coverage(&vocab, &registry, &refs);
    assert!(
        findings.is_empty(),
        "{} coverage finding(s):\n{}",
        findings.len(),
        findings.join("\n")
    );
}

#[test]
fn every_full_tool_declares_its_coverage() {
    use memstead_mcp::coverage::full_registry;
    use memstead_mcp::server::McpServer;
    let names: Vec<String> = McpServer::tool_router()
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_covered(names, full_registry());
}
