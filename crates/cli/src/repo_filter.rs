//! Team-org repo filter (architecture.md §6.1: "team org へは端末側の「リポジトリ
//! パターン許可リスト」に一致する repo のイベントだけ送信 (混在マシンで私用リポジトリが
//! 会社 org に流れない)。パターン外は personal org or ローカルのみ").
//!
//! Applied by `kikimimi agent` (`agent.rs`) **only to the cloud sink**, and **only when the
//! device's org is `org_kind == "team"`** -- the file sink (and any BYO sink) always receive
//! every event unfiltered, so nothing is ever lost locally, only kept out of the shared team
//! cloud. A personal org is never filtered, even if `repo_patterns` happens to be non-empty
//! (e.g. left over from a previous team login) -- filtering is explicitly opt-in per §6.1's
//! "team org へは...だけ送信", not a general-purpose privacy knob for personal orgs.
//!
//! `event.repo` (`kikimimi_schema::Event::repo`) is only populated by adapters/paths that
//! can derive it: the Codex rollout tailer (from the session's `git.repository_url`), and
//! since issue #4, Claude Code hook events (`agent.rs`'s `drain_spool`, derived from the
//! hook payload's `cwd` via `repo_resolve::RepoResolver` reading `.git/config` -- no repo
//! is set when the hook's `cwd` isn't inside a git working tree, or has no remote). OTel
//! events carry no `cwd` and still never set it. An event with `repo = None` on a team org
//! with a non-empty allowlist is treated as **not matching** -- conservative by design: we
//! can't confirm it belongs to an allowed repo, and the whole point of this filter is keeping
//! ambiguous/unrelated events (which could be from a private, unrelated repo, or no repo at
//! all) out of the company's cloud by default.

use crate::config::CloudConfig;

/// Team-org repo allowlist, resolved once at daemon startup (and again on `kikimimi repos
/// allow/remove`'s live-reload signal, agent.rs's control byte `b'r'`) from `config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFilter {
    team: bool,
    patterns: Vec<String>,
}

impl RepoFilter {
    /// Builds the filter from the same `cloud` section of `config.json` that
    /// `CloudSink`/`kikimimi login` use. `None` (never logged in) and any `org_kind` other
    /// than exactly `"team"` (personal, or empty/unknown from an old config.json) both
    /// resolve to "no filtering" (`allows` always returns `true`).
    pub fn from_cloud_config(cloud: Option<&CloudConfig>) -> Self {
        match cloud {
            Some(c) if c.org_kind == "team" => Self {
                team: true,
                patterns: c.repo_patterns.clone(),
            },
            _ => Self::default(),
        }
    }

    /// Whether `repo` should be pushed to the **cloud** sink. Always `true` outside a team
    /// org, and always `true` for a team org with no patterns configured yet (§6.1:
    /// "empty/absent patterns = send everything") -- callers must pair an empty-patterns
    /// team org with [`Self::unconfigured_warning`] so the operator knows everything is
    /// currently unfiltered.
    pub fn allows(&self, repo: Option<&str>) -> bool {
        if !self.team || self.patterns.is_empty() {
            return true;
        }
        matches_any(&self.patterns, repo)
    }

    pub fn is_team(&self) -> bool {
        self.team
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// A one-time (caller decides when "once" is -- agent.rs calls this only right at
    /// startup, not on every event) stderr warning for a team org with no repo patterns
    /// configured: every event, including ones with no repo at all, is currently being sent
    /// to the team cloud unfiltered. Returns `None` (nothing to warn about) for a personal
    /// org, an org with any patterns configured, or when not logged in at all.
    ///
    /// A pure function returning the message (rather than printing directly) so it has a
    /// plain, deterministic unit test independent of stderr capture.
    pub fn unconfigured_warning(&self, org_slug: &str) -> Option<String> {
        if self.team && self.patterns.is_empty() {
            Some(format!(
                "kikimimi agent: team org {org_slug:?} has no repo filter configured \
                 (cloud.repo_patterns) -- every event, including from repos unrelated to this \
                 team, will be pushed to the team cloud. Run `kikimimi repos allow <glob>` to \
                 restrict what gets sent (see `kikimimi repos list`)."
            ))
        } else {
            None
        }
    }
}

/// `repo` matches if it's `Some` and equals (via [`glob_match`]) at least one of `patterns`.
/// An empty `repo` (`None`) never matches a non-empty pattern list -- see the module docs'
/// "conservative by design" note. An empty `patterns` slice always returns `false` here (this
/// is the low-level matcher; the "empty patterns = allow everything" policy lives in
/// [`RepoFilter::allows`], not here).
pub fn matches_any(patterns: &[String], repo: Option<&str>) -> bool {
    let Some(repo) = repo else {
        return false;
    };
    patterns.iter().any(|p| glob_match(p, repo))
}

/// Minimal glob matcher: `*` matches any run of characters (including none), `?` matches
/// exactly one character, everything else matches literally. Case-sensitive (repo strings are
/// typically `host/org/repo`-shaped paths or URLs, where case is meaningful). No brace/bracket
/// expansion, no `**` distinction from `*` -- deliberately small rather than pulling in a glob
/// crate for a single-column, single-purpose match.
///
/// Classic O(len(pattern) * len(text)) DP: `dp[i][j]` = pattern's first `i` chars match text's
/// first `j` chars.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (plen, tlen) = (p.len(), t.len());

    let mut dp = vec![vec![false; tlen + 1]; plen + 1];
    dp[0][0] = true;
    for i in 1..=plen {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=plen {
        for j in 1..=tlen {
            dp[i][j] = match p[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == t[j - 1],
            };
        }
    }
    dp[plen][tlen]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- glob_match ------------------------------------------------------------

    #[test]
    fn glob_match_exact_literal() {
        assert!(glob_match("github.com/acme/api", "github.com/acme/api"));
        assert!(!glob_match("github.com/acme/api", "github.com/acme/web"));
    }

    #[test]
    fn glob_match_star_matches_any_run_including_empty() {
        assert!(glob_match("github.com/acme/*", "github.com/acme/api"));
        assert!(glob_match("github.com/acme/*", "github.com/acme/"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything/at/all"));
    }

    #[test]
    fn glob_match_star_in_the_middle_and_multiple_stars() {
        assert!(glob_match(
            "github.com/*/internal-*",
            "github.com/acme/internal-tools"
        ));
        assert!(!glob_match(
            "github.com/*/internal-*",
            "github.com/acme/public-tools"
        ));
    }

    #[test]
    fn glob_match_question_mark_matches_exactly_one_char() {
        assert!(glob_match("repo-?", "repo-1"));
        assert!(!glob_match("repo-?", "repo-12"));
        assert!(!glob_match("repo-?", "repo-"));
    }

    #[test]
    fn glob_match_is_case_sensitive() {
        assert!(!glob_match("github.com/Acme/*", "github.com/acme/api"));
    }

    #[test]
    fn glob_match_empty_pattern_only_matches_empty_text() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    // --- matches_any -------------------------------------------------------------

    #[test]
    fn matches_any_true_when_any_pattern_matches() {
        let patterns = vec![
            "github.com/acme/*".to_string(),
            "github.com/other/*".to_string(),
        ];
        assert!(matches_any(&patterns, Some("github.com/acme/api")));
    }

    #[test]
    fn matches_any_false_when_no_pattern_matches() {
        let patterns = vec!["github.com/acme/*".to_string()];
        assert!(!matches_any(&patterns, Some("github.com/someone-else/api")));
    }

    #[test]
    fn matches_any_false_for_none_repo_even_with_patterns() {
        let patterns = vec!["github.com/acme/*".to_string()];
        assert!(!matches_any(&patterns, None));
    }

    #[test]
    fn matches_any_false_for_empty_pattern_list() {
        assert!(!matches_any(&[], Some("github.com/acme/api")));
    }

    // --- RepoFilter::allows --------------------------------------------------------

    fn team_cloud(patterns: Vec<&str>) -> CloudConfig {
        CloudConfig {
            org_kind: "team".to_string(),
            org_slug: "acme".to_string(),
            repo_patterns: patterns.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn allows_everything_when_not_logged_in() {
        let filter = RepoFilter::from_cloud_config(None);
        assert!(filter.allows(Some("github.com/anyone/anything")));
        assert!(filter.allows(None));
    }

    #[test]
    fn allows_everything_for_a_personal_org_regardless_of_patterns() {
        let cloud = CloudConfig {
            org_kind: "personal".to_string(),
            repo_patterns: vec!["github.com/acme/*".to_string()],
            ..Default::default()
        };
        let filter = RepoFilter::from_cloud_config(Some(&cloud));
        assert!(!filter.is_team());
        // Even a repo that wouldn't match the (irrelevant, for personal) pattern is allowed.
        assert!(filter.allows(Some("github.com/someone-else/api")));
        assert!(filter.allows(None));
    }

    #[test]
    fn allows_everything_for_team_org_with_no_patterns_configured() {
        let cloud = team_cloud(vec![]);
        let filter = RepoFilter::from_cloud_config(Some(&cloud));
        assert!(filter.is_team());
        assert!(filter.allows(Some("github.com/anyone/anything")));
        assert!(filter.allows(None));
    }

    #[test]
    fn team_org_with_patterns_only_allows_matching_repos() {
        let cloud = team_cloud(vec!["github.com/acme/*"]);
        let filter = RepoFilter::from_cloud_config(Some(&cloud));
        assert!(filter.allows(Some("github.com/acme/api")));
        assert!(!filter.allows(Some("github.com/someone-else/api")));
    }

    #[test]
    fn team_org_with_patterns_rejects_unknown_repo() {
        // Conservative-by-design: an event with no repo info at all must not sneak through a
        // configured team-org allowlist just because there was nothing to check against.
        let cloud = team_cloud(vec!["github.com/acme/*"]);
        let filter = RepoFilter::from_cloud_config(Some(&cloud));
        assert!(!filter.allows(None));
    }

    #[test]
    fn org_kind_other_than_team_is_never_filtered() {
        // Old config.json (pre-account-model) has org_kind = "" -- must behave like personal,
        // never like team, even if repo_patterns somehow got populated.
        let cloud = CloudConfig {
            org_kind: String::new(),
            repo_patterns: vec!["github.com/acme/*".to_string()],
            ..Default::default()
        };
        let filter = RepoFilter::from_cloud_config(Some(&cloud));
        assert!(!filter.is_team());
        assert!(filter.allows(Some("github.com/someone-else/api")));
    }

    // --- unconfigured_warning -------------------------------------------------------

    #[test]
    fn unconfigured_warning_present_only_for_team_with_no_patterns() {
        assert!(RepoFilter::from_cloud_config(Some(&team_cloud(vec![])))
            .unconfigured_warning("acme")
            .is_some());
    }

    #[test]
    fn unconfigured_warning_absent_once_patterns_are_configured() {
        assert!(
            RepoFilter::from_cloud_config(Some(&team_cloud(vec!["github.com/acme/*"])))
                .unconfigured_warning("acme")
                .is_none()
        );
    }

    #[test]
    fn unconfigured_warning_absent_for_personal_org() {
        let cloud = CloudConfig {
            org_kind: "personal".to_string(),
            ..Default::default()
        };
        assert!(RepoFilter::from_cloud_config(Some(&cloud))
            .unconfigured_warning("me")
            .is_none());
    }

    #[test]
    fn unconfigured_warning_mentions_the_org_slug_and_how_to_fix_it() {
        let warning = RepoFilter::from_cloud_config(Some(&team_cloud(vec![])))
            .unconfigured_warning("acme")
            .unwrap();
        assert!(warning.contains("acme"));
        assert!(warning.contains("kikimimi repos allow"));
    }
}
