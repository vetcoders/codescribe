//! Word-level sequence alignment (LCS backtrace) for Teacher diffs.

use super::tokenize::Token;

/// One step of the alignment between two token sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignOp {
    /// Tokens match (normalized).
    Equal { a: usize, b: usize },
    /// Present only on side A (e.g. live-only / apple-only).
    DeleteA { a: usize },
    /// Present only on side B (e.g. whisper-only excess).
    InsertB { b: usize },
    /// Both sides have a token but norms differ.
    Substitute { a: usize, b: usize },
}

/// Align `a` (live/Apple) to `b` (Whisper) with classic DP LCS + greedy substitute.
pub fn align_words(a: &[Token], b: &[Token]) -> Vec<AlignOp> {
    let n = a.len();
    let m = b.len();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if a[i].norm == b[j].norm {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    let mut ops = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < n && j < m {
        if a[i].norm == b[j].norm {
            ops.push(AlignOp::Equal { a: i, b: j });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(AlignOp::DeleteA { a: i });
            i += 1;
        } else {
            ops.push(AlignOp::InsertB { b: j });
            j += 1;
        }
    }
    while i < n {
        ops.push(AlignOp::DeleteA { a: i });
        i += 1;
    }
    while j < m {
        ops.push(AlignOp::InsertB { b: j });
        j += 1;
    }

    coalesce_substitutes(ops)
}

fn coalesce_substitutes(ops: Vec<AlignOp>) -> Vec<AlignOp> {
    let mut out = Vec::with_capacity(ops.len());
    let mut idx = 0;
    while idx < ops.len() {
        match (&ops[idx], ops.get(idx + 1)) {
            (AlignOp::DeleteA { a }, Some(AlignOp::InsertB { b })) => {
                out.push(AlignOp::Substitute { a: *a, b: *b });
                idx += 2;
            }
            (AlignOp::InsertB { b }, Some(AlignOp::DeleteA { a })) => {
                out.push(AlignOp::Substitute { a: *a, b: *b });
                idx += 2;
            }
            _ => {
                out.push(ops[idx].clone());
                idx += 1;
            }
        }
    }
    out
}
