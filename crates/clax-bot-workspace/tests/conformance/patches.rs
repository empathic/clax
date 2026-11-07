//! `repo::Clone::apply` as whoever controls a container and its model sees
//! it: every patch here is bytes they chose, and the host's clone must end
//! as it began unless the patch is an ordinary change.

use crate::fixture::{git, seed};
use clax_bot_workspace::repo::{Clone, Policy};
use std::path::Path;
use tempfile::TempDir;

struct Host {
    root: TempDir,
    clone: Clone,
}

impl Host {
    fn dir(&self) -> std::path::PathBuf {
        self.root.path().join("host/repo")
    }

    async fn apply(&self, patch: &str) -> Option<String> {
        self.clone
            .apply_onto("HEAD", patch.as_bytes(), &Policy::default())
            .await
            .unwrap()
    }

    /// The path `path` in the index, as bytes.
    fn staged(&self, path: &str) -> String {
        git(&self.dir(), &["show", &format!(":{path}")])
    }

    /// Fails unless `patch` is refused and leaves the clone clean.
    async fn refuses(&self, patch: &str) -> String {
        let why = self.apply(patch).await;
        let why = why.unwrap_or_else(|| panic!("applied:\n{patch}"));
        let dirty = git(&self.dir(), &["diff", "--cached", "--name-only", "HEAD"]);
        assert_eq!(dirty, "", "left behind by: {patch}");
        why
    }
}

async fn host() -> Host {
    let root = tempfile::tempdir().unwrap();
    let bare = seed(root.path());
    std::fs::create_dir(root.path().join("host")).unwrap();
    let clone = Clone::new(
        &format!("file://{}", bare.display()),
        &root.path().join("host/repo"),
        None,
    )
    .await
    .unwrap();
    clone.start_branch("claxbot/t", "main").await.unwrap();
    Host { root, clone }
}

fn new_file(path: &str, mode: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\nnew file mode {mode}\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+x\n"
    )
}

#[tokio::test]
async fn a_new_path_attribute_in_rust_is_refused_however_the_patch_carries_it() {
    let h = host().await;
    let policy = Policy {
        refused_in_rust: vec!["#[path".to_string()],
        ..Policy::default()
    };
    let patch = "diff --git a/m.rs b/m.rs\nnew file mode 100644\n--- /dev/null\n+++ b/m.rs\n\
                 @@ -0,0 +1 @@\n+#[path = \"../protected.rs\"] mod p;\n";
    let why = h.clone.apply_onto("HEAD", patch.as_bytes(), &policy).await;
    let why = why.unwrap().unwrap();
    assert!(why.contains("m.rs") && why.contains("#[path"), "{why}");
    let text = patch.replace("m.rs", "m.txt");
    let why = h.clone.apply_onto("HEAD", text.as_bytes(), &policy).await;
    assert_eq!(why.unwrap(), None, "only Rust reads it");
}

#[tokio::test]
async fn a_mode_line_that_is_not_text_is_refused_as_any_other() {
    let h = host().await;
    let mut patch = new_file("made", "040000 ~").into_bytes();
    let at = patch.iter().position(|b| *b == b'~').unwrap();
    patch[at] = 0xff;
    let policy = Policy::default();
    let why = h.clone.apply_onto("HEAD", &patch, &policy);
    let why = why.await.unwrap().expect("it was refused");
    assert!(why.contains("mode"), "{why}");
    let dirty = git(&h.dir(), &["diff", "--cached", "--name-only", "HEAD"]);
    assert_eq!(dirty, "");
}

#[tokio::test]
async fn an_ordinary_edit_applies() {
    let h = host().await;
    let patch = "diff --git a/app.txt b/app.txt\n--- a/app.txt\n+++ b/app.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n";
    let why = h.apply(patch).await;
    assert_eq!(why, None);
    assert_eq!(h.staged("app.txt"), "one\n2\nthree");
}

#[tokio::test]
async fn a_mode_in_a_patch_header_that_is_no_plain_file_is_refused() {
    let h = host().await;
    for mode in ["040000", "100600", "100664", "000000"] {
        let why = h.refuses(&new_file("made", mode)).await;
        assert!(why.contains("mode"), "{mode}: {why}");
    }
    let edit = "--- a/app.txt\n+++ b/app.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n";
    for header in [
        "old mode 100644\nnew mode 040000\n",
        "old mode 040000\nnew mode 100644\n",
        "index 1111111..2222222 040000\n",
    ] {
        let patch = format!("diff --git a/app.txt b/app.txt\n{header}{edit}");
        let why = h.refuses(&patch).await;
        assert!(why.contains("mode"), "{header}: {why}");
    }
    let gone = "diff --git a/app.txt b/app.txt\ndeleted file mode 040000\n--- a/app.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-one\n-two\n-three\n";
    assert!(h.refuses(gone).await.contains("mode"));
}

#[tokio::test]
async fn a_symlink_or_submodule_in_a_patch_header_keeps_its_own_words() {
    let h = host().await;
    for mode in ["120000", "160000"] {
        let why = h.refuses(&new_file("made", mode)).await;
        assert!(!why.contains("mode 1"), "{mode}: {why}");
    }
}

#[tokio::test]
async fn a_patch_cannot_leave_the_tree() {
    let h = host().await;
    for path in ["../escaped", "../../escaped", "a/../../escaped"] {
        h.refuses(&new_file(path, "100644")).await;
    }
    assert!(!h.root.path().join("host/escaped").exists());
    assert!(!h.root.path().join("escaped").exists());
}

#[tokio::test]
async fn an_absolute_path_stays_inside_the_tree() {
    let h = host().await;
    let path = "/tmp/clax-absolute-path-test";
    let patch = format!(
        "diff --git {path} {path}\nnew file mode 100644\n--- /dev/null\n+++ {path}\n@@ -0,0 +1 @@\n+x\n"
    );
    // git strips the leading slash as it would strip `a/`.
    let why = h.apply(&patch).await;
    assert_eq!(why, None);
    assert!(!Path::new(path).exists());
    assert_eq!(h.staged("tmp/clax-absolute-path-test"), "x");
}

#[tokio::test]
async fn a_patch_cannot_write_into_git_metadata() {
    let h = host().await;
    for path in [
        ".git/hooks/pre-push",
        ".git/config",
        ".GIT/config",
        ".Git/hooks/pre-commit",
        "sub/.git/config",
        "./.git/config",
    ] {
        h.refuses(&new_file(path, "100755")).await;
    }
    assert!(!h.dir().join(".git/hooks/pre-push").exists());
}

#[tokio::test]
async fn a_patch_cannot_make_a_symlink_or_a_submodule() {
    let h = host().await;
    let link = "diff --git a/link b/link\nnew file mode 120000\n--- /dev/null\n+++ b/link\n@@ -0,0 +1 @@\n+/etc/passwd\n\\ No newline at end of file\n";
    assert!(h.refuses(link).await.contains("symlink"));
    let gitlink = "diff --git a/mod b/mod\nnew file mode 160000\nindex 0000000..1111111\n--- /dev/null\n+++ b/mod\n@@ -0,0 +1 @@\n+Subproject commit 1111111111111111111111111111111111111111\n";
    h.refuses(gitlink).await;
}

#[tokio::test]
async fn a_patch_cannot_turn_a_file_into_a_symlink() {
    let h = host().await;
    let patch = "diff --git a/app.txt b/app.txt\ndeleted file mode 100644\n--- a/app.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-one\n-two\n-three\ndiff --git a/app.txt b/app.txt\nnew file mode 120000\n--- /dev/null\n+++ b/app.txt\n@@ -0,0 +1 @@\n+/etc/passwd\n\\ No newline at end of file\n";
    h.refuses(patch).await;
}

#[tokio::test]
async fn a_patch_cannot_write_through_a_symlink() {
    let h = host().await;
    // Through one it makes, and through one the repository already has.
    let made = format!(
        "diff --git a/link b/link\nnew file mode 120000\n--- /dev/null\n+++ b/link\n@@ -0,0 +1 @@\n+/tmp\n\\ No newline at end of file\n{}",
        new_file("link/escaped-through-new-link", "100644")
    );
    h.refuses(&made).await;
    h.refuses(&new_file("outside/escaped-through-old-link", "100644"))
        .await;
    assert!(!Path::new("/tmp/escaped-through-new-link").exists());
    assert!(!Path::new("/tmp/escaped-through-old-link").exists());
}

#[tokio::test]
async fn a_patch_cannot_repoint_a_symlink_the_repository_has() {
    let h = host().await;
    let patch = "diff --git a/outside b/outside\n--- a/outside\n+++ b/outside\n@@ -1 +1 @@\n-/tmp\n\\ No newline at end of file\n+/etc\n\\ No newline at end of file\n";
    assert!(h.refuses(patch).await.contains("symlink"));
}

#[tokio::test]
async fn a_patch_cannot_change_the_mode_of_a_file() {
    let h = host().await;
    let patch = "diff --git a/tool.sh b/tool.sh\nold mode 100644\nnew mode 100755\n";
    assert!(h.refuses(patch).await.contains("mode"));
}

#[tokio::test]
async fn a_new_executable_file_is_an_ordinary_change() {
    let h = host().await;
    let why = h.apply(&new_file("run.sh", "100755")).await;
    assert_eq!(why, None);
}

#[tokio::test]
async fn protected_paths_hold_whatever_their_case() {
    let h = host().await;
    for path in [
        ".github/workflows/x.yml",
        ".GitHub/workflows/x.yml",
        ".GITHUB/workflows/x.yml",
        ".GitModules",
    ] {
        assert!(
            h.refuses(&new_file(path, "100644"))
                .await
                .contains("protected"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn deleting_or_renaming_out_of_a_protected_path_is_refused() {
    let h = host().await;
    let delete = "diff --git a/.github/ci.yml b/.github/ci.yml\ndeleted file mode 100644\n--- a/.github/ci.yml\n+++ /dev/null\n@@ -1 +0,0 @@\n-on: push\n";
    assert!(h.refuses(delete).await.contains("protected"));
    let rename = "diff --git a/.github/ci.yml b/ci.yml\nsimilarity index 100%\nrename from .github/ci.yml\nrename to ci.yml\n";
    assert!(h.refuses(rename).await.contains("protected"));
}

#[tokio::test]
async fn a_binary_patch_may_not_promise_more_than_the_limit() {
    let h = host().await;
    // git allocates what `literal` declares before it inflates anything.
    let bomb = "diff --git a/big.bin b/big.bin\nnew file mode 100644\nGIT binary patch\nliteral 4294967295\nzcmV?d00001\n\nliteral 0\nHcmV?d00001\n\n";
    assert!(h.refuses(bomb).await.contains("limit"));
}

#[tokio::test]
async fn a_binary_delta_is_refused() {
    let h = host().await;
    // A small delta can expand without limit, and only inflating it shows by
    // how much.
    let delta = "diff --git a/app.txt b/app.txt\nGIT binary patch\ndelta 10\nzcmV?d00001\n\ndelta 10\nzcmV?d00001\n\n";
    assert!(h.refuses(delta).await.contains("binary"));
}

#[tokio::test]
async fn garbage_is_refused_and_named_as_not_applying() {
    let h = host().await;
    assert!(h.refuses("not a patch\n").await.contains("does not apply"));
    let clipped = format!("{}\n", "x".repeat(1 << 20));
    let why = h.refuses(&clipped).await;
    assert!(why.len() < 4096, "a refusal is clipped: {}", why.len());
}

#[tokio::test]
async fn a_small_patch_cannot_multiply_a_file_across_the_host() {
    let h = host().await;
    let mut patch = String::new();
    for i in 0..300 {
        patch.push_str(&format!(
            "diff --git a/app.txt b/copy{i}\nsimilarity index 100%\ncopy from app.txt\ncopy to copy{i}\n"
        ));
    }
    let why = h.refuses(&patch).await;
    assert!(why.contains("limit"), "{why}");
    assert!(!h.dir().join("copy0").exists());
}

#[tokio::test]
async fn an_applied_patch_writes_no_file_to_the_host() {
    let h = host().await;
    let why = h.apply(&new_file("kept.txt", "100644")).await;
    assert_eq!(why, None);
    assert!(!h.dir().join("kept.txt").exists());
}

#[tokio::test]
async fn a_copy_cannot_read_from_outside_the_tree_or_from_git_metadata() {
    let h = host().await;
    for from in ["../secret", "../../secret", ".git/config", ".GIT/config"] {
        let patch = format!(
            "diff --git a/{from} b/stolen\nsimilarity index 100%\ncopy from {from}\ncopy to stolen\n"
        );
        h.refuses(&patch).await;
    }
}

#[tokio::test]
async fn copies_under_the_file_limit_add_index_entries_and_no_bytes_to_the_host() {
    let h = host().await;
    let size = |dir: &Path| du(dir);
    let before = size(&h.dir());
    let mut patch = String::new();
    for i in 0..40 {
        patch.push_str(&format!(
            "diff --git a/app.txt b/copy{i}\nsimilarity index 100%\ncopy from app.txt\ncopy to copy{i}\n"
        ));
    }
    assert_eq!(h.apply(&patch).await, None);
    assert_eq!(h.staged("copy39"), "one\ntwo\nthree");
    assert!(!h.dir().join("copy0").exists());
    assert!(size(&h.dir()) < before + 1024 * 1024);
}

/// The bytes under `dir`.
fn du(dir: &Path) -> u64 {
    let mut total = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let meta = std::fs::symlink_metadata(e.path()).unwrap();
        total += if meta.is_dir() {
            du(&e.path())
        } else {
            meta.len()
        };
    }
    total
}

#[tokio::test]
async fn a_path_differing_only_by_case_from_one_in_the_tree_is_refused() {
    let h = host().await;
    let why = h.refuses(&new_file("APP.TXT", "100644")).await;
    assert!(why.contains("case"), "{why}");
}

#[tokio::test]
async fn attributes_are_protected_by_default() {
    let h = host().await;
    let why = h.refuses(&new_file(".gitattributes", "100644")).await;
    assert!(why.contains("protected"), "{why}");
}
