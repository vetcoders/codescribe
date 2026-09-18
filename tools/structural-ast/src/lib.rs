//! Neutral lexical evidence, never a Rust semantic or runtime proof.
//!
//! The accepted language is deliberately smaller than Rust. Control-bearing
//! ownership atoms (join, bounded drain, Option callbacks) have explicit AST
//! productions. Everything outside those productions refuses evidence. In
//! particular, arbitrary macros, calls, attributes, closures and loops do not
//! inherit the meaning of a familiar identifier. See COMPILE_EMBARGO.md.

use serde::{Deserialize, Serialize};
use syn::{Block, Expr, ImplItemFn, Stmt, parse_quote, visit::Visit};

mod finality;
mod productions;

pub const IDENTITY: &str = "codescribe-structural-ast/0.1.0;syn=2.0.118;grammar=2";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema: String,
    pub bodies: Vec<Body>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub symbol: String,
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
    pub source: String,
    pub truncated: bool,
    pub total_lines: usize,
    pub line_cap: usize,
    pub extent: String,
}

#[derive(Serialize)]
pub struct Evidence {
    pub schema: &'static str,
    pub identity: &'static str,
    pub accepted: bool,
    pub contracts: Vec<Contract>,
    pub failures: Vec<String>,
}

#[derive(Serialize)]
pub struct Contract {
    pub symbol: String,
    pub accepted: bool,
    pub events: Vec<String>,
    pub failures: Vec<String>,
}

struct Grammar {
    events: Vec<String>,
    failures: Vec<String>,
}

impl Grammar {
    fn require(&mut self, condition: bool, why: &str) {
        if !condition {
            self.failures.push(why.into());
        }
    }

    /// Compare typed AST nodes, including every nested expression and attribute.
    /// No token/string search, normalization, line order or span equality.
    fn atom(&mut self, actual: &Stmt, expected: Stmt, meaning: &str) {
        if actual == &expected {
            self.events.push(meaning.into());
        } else {
            self.failures
                .push(format!("BOUNDARY: unsupported {meaning}: {actual:?}"));
        }
    }

    fn sequence(&mut self, block: &Block, atoms: Vec<(Stmt, &str)>) {
        self.require(
            block.stmts.len() == atoms.len(),
            "BOUNDARY: extra/missing statement in ownership sequence",
        );
        for (actual, (expected, meaning)) in block.stmts.iter().zip(atoms) {
            self.atom(actual, expected, meaning);
        }
    }
}

/// Independently inventory effect sites through AST nesting, not just the
/// recognized statement productions. A closure is a deferred lexical scope:
/// it can never borrow an enclosing guard to discharge a function obligation.
#[derive(Default)]
struct PasteSites {
    focus: bool,
    preflight: bool,
    deferred: bool,
    sites: usize,
    violations: usize,
}

fn condition_guarantees(expr: &Expr, expected: &Expr) -> bool {
    if expr == expected {
        return true;
    }
    match expr {
        Expr::Paren(e) => condition_guarantees(&e.expr, expected),
        Expr::Binary(e) if matches!(e.op, syn::BinOp::And(_)) => {
            condition_guarantees(&e.left, expected) || condition_guarantees(&e.right, expected)
        }
        _ => false,
    }
}

impl<'ast> Visit<'ast> for PasteSites {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.visit_expr(&node.cond);
        let old = (self.focus, self.preflight);
        self.focus |= condition_guarantees(&node.cond, &parse_quote!(focus_confirmed));
        self.preflight |=
            condition_guarantees(&node.cond, &parse_quote!(preflight.can_post_events()));
        self.visit_block(&node.then_branch);
        (self.focus, self.preflight) = old;
        if let Some((_, otherwise)) = &node.else_branch {
            self.visit_expr(otherwise);
        }
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        let old = self.deferred;
        self.deferred = true;
        syn::visit::visit_expr_closure(self, node);
        self.deferred = old;
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if *node.func == parse_quote!(clipboard::paste_and_restore) {
            self.sites += 1;
            if !self.focus || !self.preflight || self.deferred {
                self.violations += 1;
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

fn check(body: &Body) -> Contract {
    let mut g = Grammar {
        events: vec![],
        failures: vec![],
    };
    let valid = body.language == "rs"
        && !body.truncated
        && body.extent == "brace"
        && body.start_line > 0
        && body.end_line >= body.start_line
        && body.total_lines == body.end_line - body.start_line + 1
        && body.source.lines().count() == body.total_lines
        && body.line_cap >= body.total_lines
        && body.source.len() <= 128 * 1024;
    g.require(valid, "incomplete or malformed Loctree body");
    if valid {
        match syn::parse_str::<ImplItemFn>(&body.source) {
            Err(error) => g.failures.push(format!("Rust parse error: {error}")),
            Ok(function) => {
                g.require(
                    function.sig.ident == body.symbol,
                    "body symbol/declaration mismatch",
                );
                let mut expected: ImplItemFn = match body.symbol.as_str() {
                    "paste_text_from_overlay" => parse_quote!(
                        pub async fn paste_text_from_overlay(
                            &self,
                            text: String,
                        ) -> Result<OverlayPasteResult> {
                        }
                    ),
                    "execute_clipboard_paste" => parse_quote!(
                        async fn execute_clipboard_paste(
                            &self,
                            paste_text: String,
                            target_app: Option<String>,
                            context: &'static str,
                        ) -> Result<OverlayPasteResult> {
                        }
                    ),
                    "stop" => parse_quote!(
                        pub async fn stop(
                            &mut self,
                        ) -> Result<(String, Option<std::path::PathBuf>)> {
                        }
                    ),
                    "complete_stop" => parse_quote!(
                        async fn complete_stop(
                            &mut self,
                            stopped: Result<Option<std::path::PathBuf>>,
                        ) -> Result<(String, Option<std::path::PathBuf>)> {
                        }
                    ),
                    "terminal_finality" => parse_quote!(
                        pub fn terminal_finality(
                            &self,
                            session: &str,
                            capture_epoch: u64,
                        ) -> TerminalFinality {
                        }
                    ),
                    "has_no_capture_facts" => parse_quote!(
                        pub fn has_no_capture_facts(&self) -> bool {}
                    ),
                    "matches_refused_document" => parse_quote!(
                        pub(crate) fn matches_refused_document(
                            &self,
                            refusal: &TerminalFinalityRefusal,
                            text: &str,
                        ) -> bool {
                        }
                    ),
                    "process_terminal_stop_error" => parse_quote!(
                        async fn process_terminal_stop_error<F, Fut>(
                            &self,
                            error: anyhow::Error,
                            deliver: F,
                        ) -> Result<ProcessRecordingOutcome>
                        where
                            F: FnOnce(String) -> Fut,
                            Fut: std::future::Future<Output = Result<TranscriptDelivery>>,
                        {
                        }
                    ),
                    _ => function.clone(),
                };
                // Optional trailing commas are syntax trivia, not ownership.
                let mut signature = function.sig.clone();
                signature.inputs = signature.inputs.into_iter().collect();
                expected.sig.inputs = expected.sig.inputs.into_iter().collect();
                g.require(
                    signature == expected.sig
                        && function.vis == expected.vis
                        && function.defaultness.is_none(),
                    "unsupported ownership signature",
                );
                g.require(
                    function.attrs.is_empty()
                        && function.sig.asyncness == expected.sig.asyncness
                        && function.sig.unsafety.is_none()
                        && function.sig.abi.is_none()
                        && function.sig.generics == expected.sig.generics,
                    "unsupported function attributes/signature",
                );
                match body.symbol.as_str() {
                    "paste_text_from_overlay" => productions::overlay(&mut g, &function.block),
                    "execute_clipboard_paste" => {
                        productions::paste(&mut g, &function.block);
                        let mut sites = PasteSites::default();
                        sites.visit_block(&function.block);
                        g.require(sites.sites == 1, "expected exactly one paste effect site");
                        g.require(
                            sites.violations == 0,
                            "paste site lacks focus/preflight dominance or is deferred",
                        );
                        g.events.push(format!(
                            "paste_sites={};unguarded={}",
                            sites.sites, sites.violations
                        ));
                    }
                    "stop" => productions::stop(&mut g, &function.block),
                    "complete_stop" => productions::complete(&mut g, &function.block),
                    "terminal_finality" => finality::ledger(&mut g, &function.block),
                    "has_no_capture_facts" => finality::empty(&mut g, &function.block),
                    "matches_refused_document" => finality::bus(&mut g, &function.block),
                    "process_terminal_stop_error" => finality::controller(&mut g, &function.block),
                    _ => g.failures.push("unknown contract".into()),
                }
            }
        }
    }
    Contract {
        symbol: body.symbol.clone(),
        accepted: g.failures.is_empty(),
        events: g.events,
        failures: g.failures,
    }
}

pub fn analyze(request: Request) -> Evidence {
    let expected = [
        ("paste_text_from_overlay", "app/controller/mod.rs"),
        ("execute_clipboard_paste", "app/controller/mod.rs"),
        ("stop", "core/audio/streaming_recorder.rs"),
        ("complete_stop", "core/audio/streaming_recorder.rs"),
        ("terminal_finality", "core/pipeline/acoustic_ledger.rs"),
        ("has_no_capture_facts", "core/pipeline/acoustic_ledger.rs"),
        (
            "matches_refused_document",
            "app/presentation/transcript_bus.rs",
        ),
        ("process_terminal_stop_error", "app/controller/mod.rs"),
    ];
    let mut failures = Vec::new();
    if request.schema != "codescribe.structural-ast-input.v1"
        || request.bodies.len() != expected.len()
    {
        failures.push("input schema or body cardinality mismatch".into());
    }
    for (symbol, file) in expected {
        if request
            .bodies
            .iter()
            .filter(|body| body.symbol == symbol && body.file == file)
            .count()
            != 1
        {
            failures.push(format!("expected unique {file}::{symbol}"));
        }
    }
    let contracts: Vec<_> = request.bodies.iter().map(check).collect();
    Evidence {
        schema: "codescribe.structural-ast-evidence.v1",
        identity: IDENTITY,
        accepted: failures.is_empty() && contracts.iter().all(|contract| contract.accepted),
        contracts,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites(block: Block) -> PasteSites {
        let mut sites = PasteSites::default();
        sites.visit_block(&block);
        sites
    }

    #[test]
    fn nested_guards_establish_lexical_dominance() {
        let evidence = sites(parse_quote!({
            if focus_confirmed {
                if preflight.can_post_events() {
                    clipboard::paste_and_restore(&text);
                }
            }
        }));
        assert_eq!((evidence.sites, evidence.violations), (1, 0));
    }

    #[test]
    fn else_never_inherits_then_guard() {
        let evidence = sites(parse_quote!({
            if focus_confirmed && preflight.can_post_events() {
            } else {
                clipboard::paste_and_restore(&text);
            }
        }));
        assert_eq!((evidence.sites, evidence.violations), (1, 1));
    }

    #[test]
    fn closure_cannot_borrow_enclosing_guard() {
        let evidence = sites(parse_quote!({
            if focus_confirmed && preflight.can_post_events() {
                let later = || clipboard::paste_and_restore(&text);
            }
        }));
        assert_eq!((evidence.sites, evidence.violations), (1, 1));
    }

    #[test]
    fn duplicate_effect_and_or_condition_are_visible() {
        let evidence = sites(parse_quote!({
            if focus_confirmed || preflight.can_post_events() {
                clipboard::paste_and_restore(&text);
            }
            clipboard::paste_and_restore(&text);
        }));
        assert_eq!((evidence.sites, evidence.violations), (2, 2));
    }

    #[test]
    fn comment_does_not_create_effect() {
        let evidence = sites(parse_quote!({
            let text = "clipboard::paste_and_restore(&text)";
        }));
        assert_eq!(evidence.sites, 0);
    }

    #[test]
    fn malformed_schema_cannot_claim_success() {
        let evidence = analyze(Request {
            schema: "unknown".into(),
            bodies: vec![],
        });
        assert!(!evidence.accepted);
        assert!(!evidence.failures.is_empty());
    }
}
