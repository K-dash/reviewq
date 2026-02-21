//! Rule engine for filtering PRs.
//!
//! Evaluates: allowlist, open-only, non-draft, skip-self-authored, reviewer-check.

use crate::types::{PrState, PullRequest, RepoId};

/// Check if a PR passes all filtering rules.
///
/// Both `skip_self_authored` and `skip_reviewer_check` are resolved per-repo.
pub fn should_process(
    pr: &PullRequest,
    username: &str,
    allowlist: &[RepoId],
    skip_self_authored: bool,
    skip_reviewer_check: bool,
) -> bool {
    is_in_allowlist(pr, allowlist)
        && is_open(pr)
        && !is_draft(pr)
        && (!skip_self_authored || !is_self_authored(pr, username))
        && (skip_reviewer_check || is_review_requested(pr, username))
}

fn is_in_allowlist(pr: &PullRequest, allowlist: &[RepoId]) -> bool {
    allowlist.iter().any(|r| r == &pr.repo)
}

fn is_open(pr: &PullRequest) -> bool {
    pr.state == PrState::Open
}

fn is_draft(pr: &PullRequest) -> bool {
    pr.draft
}

fn is_self_authored(pr: &PullRequest, username: &str) -> bool {
    pr.author == username
}

fn is_review_requested(pr: &PullRequest, username: &str) -> bool {
    pr.requested_reviewers.iter().any(|r| r == username)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pr() -> PullRequest {
        PullRequest {
            repo: RepoId::new("org", "repo"),
            number: 1,
            url: "https://github.com/org/repo/pull/1".into(),
            head_sha: "abc123".into(),
            author: "alice".into(),
            requested_reviewers: vec!["bob".into()],
            state: PrState::Open,
            draft: false,
        }
    }

    fn allowlist() -> Vec<RepoId> {
        vec![RepoId::new("org", "repo")]
    }

    #[test]
    fn passes_all_rules() {
        let pr = make_pr();
        assert!(should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_repo_not_in_allowlist() {
        let mut pr = make_pr();
        pr.repo = RepoId::new("other", "repo");
        assert!(!should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_closed_pr() {
        let mut pr = make_pr();
        pr.state = PrState::Closed;
        assert!(!should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_merged_pr() {
        let mut pr = make_pr();
        pr.state = PrState::Merged;
        assert!(!should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_draft_pr() {
        let mut pr = make_pr();
        pr.draft = true;
        assert!(!should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_self_authored_pr() {
        let pr = make_pr();
        assert!(!should_process(&pr, "alice", &allowlist(), true, false));
    }

    #[test]
    fn accepts_self_authored_when_skip_disabled() {
        let mut pr = make_pr();
        pr.requested_reviewers.push("alice".into());
        assert!(should_process(&pr, "alice", &allowlist(), false, false));
    }

    #[test]
    fn rejects_no_review_requested() {
        let mut pr = make_pr();
        pr.requested_reviewers.clear();
        assert!(!should_process(&pr, "bob", &allowlist(), true, false));
    }

    #[test]
    fn rejects_review_requested_for_different_user() {
        let pr = make_pr();
        assert!(!should_process(&pr, "charlie", &allowlist(), true, false));
    }

    #[test]
    fn accepts_without_reviewer_when_check_skipped() {
        let mut pr = make_pr();
        pr.requested_reviewers.clear();
        assert!(should_process(&pr, "bob", &allowlist(), true, true));
    }

    #[test]
    fn accepts_self_authored_with_both_skips() {
        let pr = make_pr();
        // author=alice, reviewer check skipped, self-authored check skipped
        assert!(should_process(&pr, "alice", &allowlist(), false, true));
    }

    #[test]
    fn allowlist_check_with_empty_list() {
        let pr = make_pr();
        assert!(!is_in_allowlist(&pr, &[]));
    }

    #[test]
    fn allowlist_check_with_multiple_repos() {
        let pr = make_pr();
        let list = vec![
            RepoId::new("other", "one"),
            RepoId::new("org", "repo"),
            RepoId::new("other", "two"),
        ];
        assert!(is_in_allowlist(&pr, &list));
    }
}
