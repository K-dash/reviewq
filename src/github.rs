//! GitHub REST API client.
//!
//! Implements [`crate::traits::GitHubClient`] using reqwest.

use reqwest::Client;
use serde::Deserialize;

use crate::error::{Result, ReviewqError};
use crate::traits::GitHubClient;
use crate::types::{PrState, PullRequest, RepoId};

const DEFAULT_BASE_URL: &str = "https://api.github.com";
const USER_AGENT: &str = "reviewq";

/// GitHub REST API client backed by reqwest.
pub struct GitHubApi {
    client: Client,
    token: String,
    base_url: String,
}

impl GitHubApi {
    pub fn new(token: String) -> Self {
        Self::with_base_url(token, DEFAULT_BASE_URL.to_owned())
    }

    /// Create a client with a custom base URL (for testing with mockito).
    pub fn with_base_url(token: String, base_url: String) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            token,
            base_url,
        }
    }

    /// Build a GET request with standard headers.
    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
    }

    /// Check the response for rate-limit and HTTP errors, returning the body on success.
    async fn check_response(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        // Check rate limit before anything else
        if resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            == Some(0)
        {
            let retry_after = resp
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|reset| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    reset.saturating_sub(now)
                })
                .unwrap_or(60);
            return Err(ReviewqError::RateLimit {
                retry_after_secs: retry_after,
            });
        }

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = resp.text().await.unwrap_or_default();
            return Err(ReviewqError::Auth(format!("GitHub API {status}: {body}")));
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ReviewqError::GitHub {
                message: format!("HTTP {status}: {body}"),
                kind: crate::error::ErrorKind::Network,
            });
        }

        Ok(resp)
    }
}

// ---------------------------------------------------------------------------
// Response types for JSON deserialization
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchResponse {
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    number: u64,
    html_url: String,
    state: String,
    draft: Option<bool>,
    user: User,
    pull_request: Option<PullRequestRef>,
    repository_url: String,
}

#[derive(Deserialize)]
struct PullRequestRef {
    html_url: String,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct PrDetailResponse {
    head: PrHead,
}

#[derive(Deserialize)]
struct PrHead {
    sha: String,
}

#[derive(Deserialize)]
struct RequestedReviewersResponse {
    users: Vec<User>,
}

#[derive(Deserialize)]
struct AuthenticatedUser {
    login: String,
}

// ---------------------------------------------------------------------------
// GitHubClient implementation
// ---------------------------------------------------------------------------

impl GitHubClient for GitHubApi {
    async fn search_review_requested(&self, repos: &[RepoId]) -> Result<Vec<PullRequest>> {
        if repos.is_empty() {
            return Ok(vec![]);
        }

        let username = self.authenticated_user().await?;

        // Build the search query
        let repo_filter: Vec<String> = repos.iter().map(|r| format!("repo:{}", r)).collect();
        let query = format!(
            "type:pr state:open review-requested:{username} {}",
            repo_filter.join(" ")
        );

        let mut all_prs = Vec::new();
        let mut page = 1u32;

        loop {
            let resp = self
                .request("/search/issues")
                .query(&[
                    ("q", query.as_str()),
                    ("per_page", "100"),
                    ("page", &page.to_string()),
                ])
                .send()
                .await?;

            let resp = self.check_response(resp).await?;
            let search: SearchResponse = resp.json().await?;

            if search.items.is_empty() {
                break;
            }

            for item in &search.items {
                // Parse repo from repository_url (e.g., "https://api.github.com/repos/owner/name")
                let repo = parse_repo_from_url(&item.repository_url);
                let Some(repo) = repo else {
                    continue;
                };

                // Fetch the PR detail to get head SHA
                let pr_resp = self
                    .request(&format!(
                        "/repos/{}/{}/pulls/{}",
                        repo.owner, repo.name, item.number
                    ))
                    .send()
                    .await?;
                let pr_resp = self.check_response(pr_resp).await?;
                let pr_detail: PrDetailResponse = pr_resp.json().await?;

                // Fetch requested reviewers for this PR
                let reviewers = self.requested_reviewers(&repo, item.number).await?;

                let url = item
                    .pull_request
                    .as_ref()
                    .map(|pr| pr.html_url.clone())
                    .unwrap_or_else(|| item.html_url.clone());

                let state = match item.state.as_str() {
                    "open" => PrState::Open,
                    "closed" => PrState::Closed,
                    _ => PrState::Closed,
                };

                all_prs.push(PullRequest {
                    repo,
                    number: item.number,
                    url,
                    head_sha: pr_detail.head.sha,
                    author: item.user.login.clone(),
                    requested_reviewers: reviewers,
                    state,
                    draft: item.draft.unwrap_or(false),
                });
            }

            // Check if there are more pages
            if search.items.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(all_prs)
    }

    async fn requested_reviewers(&self, repo: &RepoId, pr_number: u64) -> Result<Vec<String>> {
        let resp = self
            .request(&format!(
                "/repos/{}/{}/pulls/{pr_number}/requested_reviewers",
                repo.owner, repo.name
            ))
            .send()
            .await?;

        let resp = self.check_response(resp).await?;
        let data: RequestedReviewersResponse = resp.json().await?;

        Ok(data.users.into_iter().map(|u| u.login).collect())
    }

    async fn authenticated_user(&self) -> Result<String> {
        let resp = self.request("/user").send().await?;
        let resp = self.check_response(resp).await?;
        let user: AuthenticatedUser = resp.json().await?;
        Ok(user.login)
    }
}

/// Parse a `RepoId` from a GitHub API repository URL.
/// E.g., `"https://api.github.com/repos/owner/name"` -> `RepoId { owner, name }`
fn parse_repo_from_url(url: &str) -> Option<RepoId> {
    let parts: Vec<&str> = url.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        let name = parts[0];
        let owner = parts[1];
        Some(RepoId::new(owner, name))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repo_from_api_url() {
        let url = "https://api.github.com/repos/octocat/hello-world";
        let repo = parse_repo_from_url(url).expect("should parse");
        assert_eq!(repo.owner, "octocat");
        assert_eq!(repo.name, "hello-world");
    }

    #[test]
    fn parse_repo_from_custom_url() {
        let url = "http://localhost:8080/repos/owner/repo";
        let repo = parse_repo_from_url(url).expect("should parse");
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.name, "repo");
    }

    #[tokio::test]
    async fn authenticated_user_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/user")
            .match_header("Authorization", "Bearer test-token")
            .match_header("Accept", "application/vnd.github+json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"login": "testuser"}"#)
            .create_async()
            .await;

        let api = GitHubApi::with_base_url("test-token".into(), server.url());
        let user = api.authenticated_user().await.expect("should succeed");
        assert_eq!(user, "testuser");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn authenticated_user_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/user")
            .with_status(401)
            .with_body(r#"{"message": "Bad credentials"}"#)
            .create_async()
            .await;

        let api = GitHubApi::with_base_url("bad-token".into(), server.url());
        let err = api.authenticated_user().await.unwrap_err();
        assert!(matches!(err, ReviewqError::Auth(_)));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn requested_reviewers_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/repos/owner/repo/pulls/1/requested_reviewers")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"users": [{"login": "reviewer1"}, {"login": "reviewer2"}], "teams": []}"#,
            )
            .create_async()
            .await;

        let api = GitHubApi::with_base_url("token".into(), server.url());
        let repo = RepoId::new("owner", "repo");
        let reviewers = api
            .requested_reviewers(&repo, 1)
            .await
            .expect("should succeed");
        assert_eq!(reviewers, vec!["reviewer1", "reviewer2"]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn rate_limit_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/user")
            .with_status(200)
            .with_header("x-ratelimit-remaining", "0")
            .with_header("x-ratelimit-reset", "9999999999")
            .with_body(r#"{"login": "user"}"#)
            .create_async()
            .await;

        let api = GitHubApi::with_base_url("token".into(), server.url());
        let err = api.authenticated_user().await.unwrap_err();
        assert!(matches!(err, ReviewqError::RateLimit { .. }));
        mock.assert_async().await;
    }
}
