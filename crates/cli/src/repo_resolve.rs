//! Claude Code hook イベントの `cwd` から git remote URL を解決する (issue #4)。
//!
//! `git` をサブプロセスとして起動せず、`.git/config` (worktree の場合は `.git` ファイル
//! → `gitdir:` → (あれば) `commondir` → `config`) を直接読んで `[remote "origin"]` の
//! `url`（無ければ最初に見つかった `[remote "..."]` の `url`）を返す。値は Codex アダプタ
//! (`adapter-codex/src/rollout.rs` の `git.repository_url`) と同じ規約で、URL 文字列を
//! 加工せずそのまま格納する。
//!
//! デーモンの drain ループから毎ティック呼ばれるため、fail-open (io エラー・パース失敗は
//! すべて `None`、絶対に panic しない) かつブロッキングしない (同期 fs read のみ、
//! サブプロセスなし)ことを保証する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// キャッシュの上限エントリ数。超えたら単純に `clear()` する — LRU 等の複雑な実装はせず、
/// 1 ホストで観測される cwd の種類は通常たかだか数十〜数百なので、上限に当たること自体が
/// 想定外。ここに達したら丸ごと作り直す方が実装をシンプルに保てる。
const MAX_CACHE_ENTRIES: usize = 1024;

/// `cwd` (hook payload の生の文字列) → git remote URL のキャッシュ。
/// キーは canonicalize しない生の `PathBuf`（`kikimimi agent` プロセス内で同じ文字列の
/// cwd が来る限りヒットする。canonicalize 自体の syscall コストを避けるための割り切り）。
#[derive(Debug, Default)]
pub struct RepoResolver {
    cache: HashMap<PathBuf, Option<String>>,
}

impl RepoResolver {
    /// `cwd` から git remote URL を解決する。キャッシュヒット時は clone を返すだけ。
    /// ミス時は祖先ディレクトリを上に辿って最初に見つかった `.git` を読み、結果
    /// (`None` を含む) をキャッシュしてから返す。
    pub fn resolve(&mut self, cwd: &str) -> Option<String> {
        let key = PathBuf::from(cwd);
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }

        let resolved = find_remote_url(&key);

        if self.cache.len() >= MAX_CACHE_ENTRIES {
            self.cache.clear();
        }
        self.cache.insert(key, resolved.clone());
        resolved
    }
}

/// `cwd` から `.git` が見つかるまで祖先ディレクトリを遡る。最初に見つかった `.git` が
/// そのリポジトリのルート — git 本来の挙動と同じく、そこで remote が読めなくても
/// (or 存在しなくても) それ以上は遡らない (無関係な親リポジトリの remote を誤って
/// 拾わないため)。どの祖先にも `.git` が無ければ `None`。
fn find_remote_url(cwd: &Path) -> Option<String> {
    let mut dir = cwd;
    loop {
        let git_path = dir.join(".git");
        if git_path.exists() {
            return remote_url_from_git_path(&git_path, dir);
        }
        dir = dir.parent()?;
    }
}

/// `git_path` (`<dir>/.git`) がディレクトリなら通常のリポジトリ、ファイルなら worktree
/// (`gitdir: <path>` 形式) として扱い、それぞれの `config` を解決して remote url を読む。
fn remote_url_from_git_path(git_path: &Path, ancestor_dir: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(git_path).ok()?;
    if metadata.is_dir() {
        parse_remote_url(&git_path.join("config"))
    } else if metadata.is_file() {
        let gitdir = resolve_worktree_gitdir(git_path, ancestor_dir)?;
        let git_dir = resolve_commondir(&gitdir);
        parse_remote_url(&git_dir.join("config"))
    } else {
        None
    }
}

/// worktree の `.git` ファイルの中身 (`gitdir: <path>`) から実際の gitdir パスを取り出す。
/// `<path>` が相対パスの場合、`.git` ファイルを含むディレクトリ (`ancestor_dir`) からの
/// 相対として解決する。
fn resolve_worktree_gitdir(git_file: &Path, ancestor_dir: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let raw = content
        .lines()
        .find_map(|l| l.trim().strip_prefix("gitdir:"))?;
    let raw = raw.trim();
    let path = Path::new(raw);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        ancestor_dir.join(path)
    })
}

/// `<gitdir>/commondir` があれば読み、そこに書かれた (main の `.git` を指す、相対または
/// 絶対の) パスを `<gitdir>` からの相対として解決する。無ければ `gitdir` 自体をそのまま
/// 使う (worktree ではない通常の `.git` ディレクトリを直接渡された場合を含む)。
fn resolve_commondir(gitdir: &Path) -> PathBuf {
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(content) => {
            let raw = content.trim();
            let path = Path::new(raw);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                gitdir.join(path)
            }
        }
        Err(_) => gitdir.to_path_buf(),
    }
}

/// git config ファイルを `git` を使わず素朴にパースし、`[remote "origin"]` セクション内の
/// 最初の `url = <value>` を返す。origin が無ければ、ファイル中最初に現れた
/// `[remote "..."]` セクションの `url` にフォールバックする。remote が一つも無ければ
/// `None`。値は trim するのみで、それ以外の加工はしない (Codex アダプタの
/// `git.repository_url` と同じ「生の URL 文字列をそのまま格納する」規約)。
fn parse_remote_url(config_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;

    let mut origin_url: Option<String> = None;
    let mut first_remote_url: Option<String> = None;
    let mut in_origin = false;
    let mut in_any_remote = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
            in_any_remote = trimmed.starts_with("[remote \"");
            continue;
        }
        let Some(eq_idx) = trimmed.find('=') else {
            continue;
        };
        if trimmed[..eq_idx].trim() != "url" {
            continue;
        }
        let value = trimmed[eq_idx + 1..].trim().to_string();
        if in_origin && origin_url.is_none() {
            origin_url = Some(value.clone());
        }
        if in_any_remote && first_remote_url.is_none() {
            first_remote_url = Some(value);
        }
    }

    origin_url.or(first_remote_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_config(git_dir: &Path, body: &str) {
        fs::create_dir_all(git_dir).unwrap();
        fs::write(git_dir.join("config"), body).unwrap();
    }

    #[test]
    fn resolves_origin_url_from_normal_repo() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir.path().join(".git"),
            "[core]\n    repositoryformatversion = 0\n\
             [remote \"origin\"]\n    url = https://github.com/isamisushi/guru.git\n    \
             fetch = +refs/heads/*:refs/remotes/origin/*\n",
        );

        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(dir.path().to_str().unwrap());
        assert_eq!(
            repo.as_deref(),
            Some("https://github.com/isamisushi/guru.git")
        );
    }

    #[test]
    fn resolves_from_nested_subdir_by_walking_up() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir.path().join(".git"),
            "[remote \"origin\"]\n    url = git@github.com:acme/widgets.git\n",
        );
        let nested = dir.path().join("src").join("nested");
        fs::create_dir_all(&nested).unwrap();

        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(nested.to_str().unwrap());
        assert_eq!(repo.as_deref(), Some("git@github.com:acme/widgets.git"));
    }

    #[test]
    fn resolves_through_worktree_gitdir_file_and_commondir() {
        let dir = tempfile::tempdir().unwrap();
        let main_git = dir.path().join("main-repo").join(".git");
        write_config(
            &main_git,
            "[remote \"origin\"]\n    url = https://github.com/acme/main.git\n",
        );
        let wt_gitdir = main_git.join("worktrees").join("feature");
        fs::create_dir_all(&wt_gitdir).unwrap();
        fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();

        let worktree_dir = dir.path().join("feature-worktree");
        fs::create_dir_all(&worktree_dir).unwrap();
        fs::write(
            worktree_dir.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .unwrap();

        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(worktree_dir.to_str().unwrap());
        assert_eq!(repo.as_deref(), Some("https://github.com/acme/main.git"));
    }

    #[test]
    fn falls_back_to_first_remote_when_no_origin_section() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir.path().join(".git"),
            "[remote \"upstream\"]\n    url = https://github.com/acme/upstream.git\n\
             [remote \"fork\"]\n    url = https://github.com/acme/fork.git\n",
        );

        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(dir.path().to_str().unwrap());
        assert_eq!(
            repo.as_deref(),
            Some("https://github.com/acme/upstream.git"),
            "no [remote \"origin\"] -> fall back to the first [remote \"...\"] section"
        );
    }

    #[test]
    fn returns_none_when_git_dir_has_no_remote() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            &dir.path().join(".git"),
            "[core]\n    repositoryformatversion = 0\n",
        );

        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(dir.path().to_str().unwrap());
        assert_eq!(repo, None);
    }

    #[test]
    fn returns_none_when_no_git_dir_found() {
        let dir = tempfile::tempdir().unwrap();
        let mut resolver = RepoResolver::default();
        let repo = resolver.resolve(dir.path().to_str().unwrap());
        assert_eq!(repo, None);
    }

    #[test]
    fn cache_returns_same_value_on_second_call_without_rereading_disk() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        write_config(
            &git_dir,
            "[remote \"origin\"]\n    url = https://github.com/acme/cached.git\n",
        );

        let mut resolver = RepoResolver::default();
        let cwd = dir.path().to_str().unwrap();
        let first = resolver.resolve(cwd);
        // Prove the second call comes from the cache, not a fresh read: the config file
        // that produced `first` no longer exists by the time we ask again.
        fs::remove_file(git_dir.join("config")).unwrap();
        let second = resolver.resolve(cwd);

        assert_eq!(first, second);
        assert_eq!(
            second.as_deref(),
            Some("https://github.com/acme/cached.git")
        );
    }
}
