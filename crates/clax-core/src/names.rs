//! Agent names (spec 2026-10-08-multi-agent-multi-person §11, M22, R3):
//! their form, how one is derived from a repository and branch, the words
//! and people's names no agent may take, and the token form of a person's
//! display name that both are compared in.

use std::path::Path;

/// The longest agent name, in characters (all ASCII).
pub const MAX_NAME: usize = 32;

/// Names no agent takes (spec §11): `@agent` keeps its meaning, and the
/// others name people or Clax itself.
pub const RESERVED: [&str; 5] = ["agent", "owner", "clax", "me", "everyone"];

/// Whether `name` has an agent name's form (M22):
/// `[a-z0-9][a-z0-9._-]{0,31}`.
pub fn is_valid(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|&c| name_byte(c))
}

/// A byte an agent name may hold.
fn name_byte(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
}

/// `s` made a name as §11 derives one: lowercased, every character other
/// than `[a-z0-9._-]` made `-`, runs of `-` collapsed, cut to
/// [`MAX_NAME`], and without leading or trailing `.`, `_` or `-` (a name
/// starts with a letter or digit, and a trailing one would join the
/// punctuation after an `@name`). `None` when nothing is left.
pub fn sanitize(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len().min(4 * MAX_NAME));
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii() && name_byte(c as u8) {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
        if out.len() > 2 * MAX_NAME {
            // Enough to cut from once the leading punctuation is gone.
            if out.trim_start_matches(['-', '.', '_']).len() > MAX_NAME {
                break;
            }
        }
    }
    let t = out.trim_start_matches(['-', '.', '_']);
    let t = &t[..t.len().min(MAX_NAME)];
    let t = t.trim_end_matches(['-', '.', '_']);
    (!t.is_empty()).then(|| t.to_string())
}

/// The name a session is given on registration (§11), before collisions:
/// `<repo>` when the branch is none, `main` or `master`, else
/// `<repo>-<the branch's last path segment>`, made a name by [`sanitize`];
/// with no repository, or one that leaves nothing, the harness.
pub fn derive(repo: Option<&str>, branch: Option<&str>, harness: &str) -> String {
    let base = repo.map(
        |r| match branch.map(|b| b.rsplit('/').next().unwrap_or(b)) {
            None | Some("main" | "master") => r.to_string(),
            Some(seg) => format!("{r}-{seg}"),
        },
    );
    base.as_deref()
        .and_then(sanitize)
        .or_else(|| sanitize(harness))
        .unwrap_or_else(|| "agent-session".to_string())
}

/// The candidates for a session's derived name, in order (R3): `base`,
/// then `<base>-<harness>` (unless `base` is the harness), then
/// `<base>-2`, `<base>-3`, …; each cut so it fits [`MAX_NAME`].
pub fn candidates<'a>(base: &'a str, harness: &'a str) -> impl Iterator<Item = String> + 'a {
    let with = move |suffix: &str| {
        let room = MAX_NAME.saturating_sub(suffix.len() + 1);
        let head = base[..base.len().min(room)].trim_end_matches(['-', '.', '_']);
        format!("{head}-{suffix}")
    };
    let by_harness = (base != harness)
        .then(|| sanitize(harness))
        .flatten()
        .map(|h| with(&h));
    std::iter::once(base.to_string())
        .chain(by_harness)
        .chain((2u32..).map(move |n| with(&n.to_string())))
}

/// A display name's token form (§11): lowercased, compatibility forms
/// folded (full-width letters, ligatures), diacritics and invisible format
/// characters removed, and each run of whitespace as one `-`. Two names
/// are the same person's name when their token forms are equal.
pub fn token_form(display_name: &str) -> String {
    use icu_normalizer::DecomposingNormalizerBorrowed;
    use icu_properties::props::{DefaultIgnorableCodePoint, GeneralCategory};
    use icu_properties::{CodePointMapData, CodePointSetData};
    // Spaces narrower than U+0020 read as no space at UI sizes (`Al ex`
    // with a hair space reads "Alex"), so they are dropped, before NFKD
    // turns them into ordinary spaces: the six-per-em (U+2006),
    // punctuation (U+2008), thin (U+2009), hair (U+200A), narrow no-break
    // (U+202F) and medium mathematical (U+205F) spaces. The four-per-em
    // space (U+2005) and wider ones are about a space's width and stay
    // spaces.
    let lower: String = display_name
        .to_lowercase()
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{2006}' | '\u{2008}'..='\u{200A}' | '\u{202F}' | '\u{205F}'
            )
        })
        .collect();
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(&lower);
    let gc = CodePointMapData::<GeneralCategory>::new();
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    let mut out = String::with_capacity(decomposed.len());
    let mut space = false;
    for c in decomposed.chars() {
        // Invisible characters (Hangul fillers, variation selectors, tag
        // characters) render as nothing.
        if ignorable.contains(c)
            || matches!(
                gc.get(c),
                GeneralCategory::NonspacingMark
                    | GeneralCategory::EnclosingMark
                    | GeneralCategory::SpacingMark
                    | GeneralCategory::Format
            )
        {
            continue;
        }
        // The braille blank renders as a space.
        if c.is_whitespace() || c == '\u{2800}' {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push('-');
            space = false;
        }
        out.extend(fold_letter(c));
    }
    out
}

/// Whether `s` holds a bidirectional embedding, override or isolate
/// control (U+202A–U+202E, U+2066–U+2069), which can make a name render as
/// another (`\u{202E}xelA` reads "Alex"). The marks (LRM, RLM, ALM) that
/// right-to-left names need are not among them; [`token_form`] drops them.
pub fn has_bidi_control(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
}

/// The UTS #39 confusable skeletons a display name is compared by (spec
/// §9.6): that of its [`token_form`] (case folded first, so `MIA` and
/// `mia` meet), and that of the token form of the skeleton of the name as
/// written (so a capital `I` meets an `l`, as in `AIex`). Two names look
/// alike when they share a key ([`CollisionKeys::meets`]). Any script, and
/// any mix of scripts, has keys: `Аlex` (Cyrillic А), `Ａｌｅｘ` and
/// `Alex\u{3164}` meet `Alex`, and the all-Cyrillic `Сосо` meets `Coco`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionKeys([String; 2]);

impl CollisionKeys {
    pub fn of(name: &str) -> CollisionKeys {
        let folded: String = unicode_security::skeleton(&token_form(name)).collect();
        let first: String = unicode_security::skeleton(name).collect();
        let written: String = unicode_security::skeleton(&token_form(&first)).collect();
        CollisionKeys([folded, written])
    }

    /// Whether the two names look alike: they share a key.
    pub fn meets(&self, other: &CollisionKeys) -> bool {
        self.0.iter().any(|k| other.0.contains(k))
    }
}

/// Whether display names `a` and `b` look alike ([`CollisionKeys`]).
pub fn look_alike(a: &str, b: &str) -> bool {
    CollisionKeys::of(a).meets(&CollisionKeys::of(b))
}

/// Letters whose diacritic is part of the letter (no decomposition), as
/// their base letters.
fn fold_letter(c: char) -> std::iter::Once<char> {
    std::iter::once(match c {
        'ı' => 'i',
        'ø' => 'o',
        'đ' | 'ð' => 'd',
        'ħ' => 'h',
        'ł' => 'l',
        'ŧ' => 't',
        _ => c,
    })
}

/// Whether no agent may take `name` (§11): a reserved word, or a name that
/// looks like one of `people`'s display names (the owner's and every
/// connected person's): see [`CollisionKeys`].
pub fn is_reserved(name: &str, people: &[String]) -> bool {
    let n = name.to_ascii_lowercase();
    let key = CollisionKeys::of(&n);
    RESERVED.contains(&n.as_str()) || people.iter().any(|p| CollisionKeys::of(p).meets(&key))
}

/// The repository `cwd` is in and its branch (§11), read from the files
/// with no `git` process. The nearest directory at or above `cwd` holding
/// `.git` (a directory, or a worktree's file naming its git directory)
/// gives the branch its `HEAD` names (`None` when detached or unreadable).
/// The repository is named after its common git directory (`git rev-parse
/// --git-common-dir`; a worktree's `commondir` file names it): the
/// directory holding that `.git`, so a worktree's agent is named after its
/// repository, not the worktree's directory. A bare repository's common
/// directory is its own name without `.git`. `(None, None)` outside any
/// repository.
pub fn repo_and_branch(cwd: &Path) -> (Option<String>, Option<String>) {
    if cwd.as_os_str().is_empty() {
        return (None, None);
    }
    for dir in cwd.ancestors() {
        let dot = dir.join(".git");
        let git_dir = if dot.is_dir() {
            Some(dot)
        } else if dot.is_file() {
            read_head_line(&dot).and_then(|l| {
                l.strip_prefix("gitdir:").map(|p| {
                    let p = Path::new(p.trim());
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        dir.join(p)
                    }
                })
            })
        } else {
            continue;
        };
        let common = git_dir.as_deref().and_then(|g| {
            let c = read_head_line(&g.join("commondir"))?;
            let c = Path::new(&c);
            let c = if c.is_absolute() {
                c.to_path_buf()
            } else {
                g.join(c)
            };
            std::fs::canonicalize(&c).ok()
        });
        let repo = match &common {
            Some(c) if c.file_name().is_some_and(|n| n == ".git") => {
                c.parent().and_then(Path::file_name)
            }
            Some(c) => c.file_name(),
            None => dir.file_name(),
        }
        .map(|n| {
            let n = n.to_string_lossy();
            n.strip_suffix(".git").unwrap_or(&n).to_string()
        })
        .filter(|n| !n.is_empty());
        let branch = git_dir
            .and_then(|g| read_head_line(&g.join("HEAD")))
            .and_then(|l| {
                l.strip_prefix("ref: refs/heads/")
                    .map(|b| b.trim().to_string())
            })
            .filter(|b| !b.is_empty());
        return (repo, branch);
    }
    (None, None)
}

/// The first line of the small file at `path` (at most 4 KiB read).
fn read_head_line(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines().next().map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_derive_from_repo_and_branch() {
        assert_eq!(derive(Some("clax"), Some("main"), "claude"), "clax");
        assert_eq!(
            derive(Some("clax"), Some("feature/Multi Person"), "codex"),
            "clax-multi-person"
        );
        assert_eq!(derive(None, None, "pi"), "pi");
        assert_eq!(derive(Some(&"x".repeat(80)), None, "claude").len(), 32);
        assert_eq!(sanitize("Fix: chart axis!"), Some("fix-chart-axis".into()));
        assert!(sanitize("---").is_none());
        assert_eq!(token_form("Mia Kovač"), "mia-kovac");
        assert!(
            is_reserved("agent", &[])
                && is_reserved("sam", &["Sam".into()])
                && !is_reserved("clax-main", &["Sam".into()])
        );
    }

    #[test]
    fn derivation_handles_master_detached_and_empty_parts() {
        assert_eq!(derive(Some("web"), Some("master"), "codex"), "web");
        assert_eq!(derive(Some("web"), None, "codex"), "web");
        assert_eq!(
            derive(Some("Web App"), Some("fix/Ünïcode"), "pi"),
            "web-app-n-code"
        );
        assert_eq!(
            derive(Some("日本"), None, "grok"),
            "grok",
            "nothing left: the harness"
        );
        assert_eq!(derive(Some("web"), Some("feat/日本"), "pi"), "web");
        assert_eq!(derive(Some(".hidden"), None, "pi"), "hidden");
        let long = derive(Some(&"ab-".repeat(40)), Some("x"), "claude");
        assert!(is_valid(&long) && long.len() <= MAX_NAME, "{long}");
    }

    #[test]
    fn sanitize_never_yields_an_invalid_name() {
        for s in [
            "",
            " ",
            "\u{0}\u{7}\n\t",
            "a\u{200B}b",
            "\u{202E}evil",
            "ＭＩＡ",
            "\u{212A}elvin",
            "Ѕаm",
            "_x_",
            "..",
            "a--b",
            &"é".repeat(500),
            &"z".repeat(10_000),
            "-._-a",
        ] {
            if let Some(n) = sanitize(s) {
                assert!(is_valid(&n), "{s:?} gave {n:?}");
            }
        }
        assert_eq!(sanitize("a--b"), Some("a-b".into()));
        assert_eq!(sanitize("a\u{200B}b"), Some("a-b".into()));
        assert_eq!(
            sanitize("Ѕаm"),
            Some("m".into()),
            "Cyrillic look-alikes never become their ASCII twins"
        );
        assert_eq!(sanitize(&"z".repeat(10_000)).unwrap().len(), MAX_NAME);
        assert_eq!(sanitize("-._-a"), Some("a".into()));
    }

    #[test]
    fn validity_is_the_m22_form() {
        for ok in ["a", "0", "web-2", "a.b_c-d", &"x".repeat(32)] {
            assert!(is_valid(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            ".a",
            "_a",
            "A",
            "web app",
            "wéb",
            "\u{212A}",
            &"x".repeat(33),
            "a/b",
            "a@b",
        ] {
            assert!(!is_valid(bad), "{bad:?}");
        }
    }

    #[test]
    fn candidates_suffix_the_harness_then_numbers_and_fit() {
        let c: Vec<String> = candidates("web", "codex").take(4).collect();
        assert_eq!(c, ["web", "web-codex", "web-2", "web-3"]);
        let c: Vec<String> = candidates("claude", "claude").take(3).collect();
        assert_eq!(c, ["claude", "claude-2", "claude-3"]);
        let base = "x".repeat(32);
        for n in candidates(&base, "claude").take(12) {
            assert!(is_valid(&n), "{n}");
        }
        let c: Vec<String> = candidates(&base, "claude").take(2).collect();
        assert_eq!(c[1], format!("{}-claude", "x".repeat(25)));
    }

    #[test]
    fn token_forms_fold_case_marks_compatibility_and_space() {
        assert_eq!(token_form("  Zoë  Ångström "), "zoe-angstrom");
        assert_eq!(token_form("ＭＩＡ"), "mia", "full-width letters");
        assert_eq!(token_form("Mi\u{200B}a"), "mia", "zero-width space");
        assert_eq!(token_form("Mi\u{0301}a"), "mia", "a combining accent");
        assert_eq!(
            token_form("Mia\u{00A0}Kovač"),
            "mia-kovac",
            "no-break space"
        );
        assert_eq!(token_form("Łukasz Øster"), "lukasz-oster");
        assert_eq!(token_form("ﬁona"), "fiona", "a ligature");
        assert_eq!(token_form("\u{212A}im"), "kim", "the Kelvin sign");
        assert_eq!(
            token_form("\u{202E}maS"),
            "mas",
            "a bidi override is dropped"
        );
        assert_ne!(
            token_form("Ѕаm"),
            token_form("Sam"),
            "Cyrillic look-alikes are other letters"
        );
    }

    #[test]
    fn invisible_and_blank_characters_vanish_from_token_forms() {
        for probe in [
            "Alex\u{3164}",
            "Alex\u{FFA0}",
            "A\u{115F}lex",
            "Alex\u{2800}",
            "Al\u{FE0F}ex",
            "Alex\u{E0041}",
            "Al\u{034F}ex",
        ] {
            assert_eq!(token_form(probe), "alex", "{probe:?}");
        }
        assert_eq!(token_form("Mia\u{2800}Kovač"), "mia-kovac");
    }

    #[test]
    fn bidi_controls_are_seen_and_marks_are_not() {
        for b in [
            "\u{202E}xelA",
            "\u{2066}Sam\u{2069}",
            "a\u{202A}b",
            "\u{2067}x",
        ] {
            assert!(has_bidi_control(b), "{b:?}");
        }
        for ok in [
            "Mia Kovač",
            "A\u{200F}lex",
            "Sam\u{061C}",
            "\u{05D3}\u{05E0}\u{05D4}\u{200F} Dana",
        ] {
            assert!(!has_bidi_control(ok), "{ok:?}");
        }
    }

    #[test]
    fn look_alike_names_meet() {
        for same in [
            "alex",
            "ALEX",
            "\u{0410}lex",
            "AIex",
            "Ａｌｅｘ",
            "Alex\u{3164}",
            "Alex\u{FFA0}",
            "A\u{115F}lex",
            "Alex\u{2800}",
            "Al\u{200B}ex",
            "Al\u{FE0F}ex",
            "Alex\u{E0041}",
            "Al\u{034F}ex",
            "A\u{200F}lex",
            "Al\u{200A}ex",
            "Al\u{2009}ex",
            "Al\u{202F}ex",
            "A\u{2006}lex",
            "Al\u{2008}ex",
            "Al\u{205F}ex",
        ] {
            assert!(look_alike(same, "Alex"), "{same:?}");
        }
        assert!(
            look_alike("\u{0421}\u{043E}\u{0441}\u{043E}", "Coco"),
            "all-Cyrillic Сосо"
        );
        assert!(look_alike("\u{0405}\u{0430}m", "Sam"));
        assert!(look_alike("MIA  KOVAC", "Mia Kovač"));
        assert!(!look_alike("Al ex", "Alex"), "an ordinary space is a space");
        assert!(
            look_alike("Mia\u{2005}Kovač", "Mia Kovač"),
            "a four-per-em space is a space"
        );
        for other in [
            "Alexa",
            "Alix",
            "Dmitri Дмитрий",
            "Ivan (Иван)",
            "محمد Ali",
            "АЛЕХ",
        ] {
            assert!(!look_alike(other, "Alex"), "{other:?}");
        }
    }

    #[test]
    fn reserved_names_cover_the_words_and_peoples_names_in_any_case() {
        for w in RESERVED {
            assert!(is_reserved(w, &[]));
            assert!(is_reserved(&w.to_uppercase(), &[]));
        }
        let people = ["Mia Kovač".to_string(), "Sam".to_string()];
        assert!(is_reserved("mia-kovac", &people));
        assert!(is_reserved("SAM", &people));
        assert!(!is_reserved("mia", &people));
        assert!(!is_reserved("sam-2", &people));
    }

    #[test]
    fn repository_and_branch_are_read_from_git_files() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("web");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/axis\n").unwrap();
        assert_eq!(
            repo_and_branch(&repo.join("src/deep")),
            (Some("web".into()), Some("feature/axis".into()))
        );
        std::fs::write(repo.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(
            repo_and_branch(&repo),
            (Some("web".into()), None),
            "detached"
        );
        // A worktree: `.git` is a file naming its git directory.
        let wt = d.path().join("web-wt");
        let gd = repo.join(".git/worktrees/web-wt");
        std::fs::create_dir_all(&gd).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(gd.join("HEAD"), "ref: refs/heads/fix\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gd.display())).unwrap();
        std::fs::write(gd.join("commondir"), "../..\n").unwrap();
        assert_eq!(
            repo_and_branch(&wt),
            (Some("web".into()), Some("fix".into())),
            "a worktree is named after its repository"
        );
        // Without a commondir file, the worktree's own directory names it.
        std::fs::remove_file(gd.join("commondir")).unwrap();
        assert_eq!(
            repo_and_branch(&wt),
            (Some("web-wt".into()), Some("fix".into()))
        );
        // A worktree of a bare repository.
        let bare = d.path().join("api.git");
        let bgd = bare.join("worktrees/feat");
        std::fs::create_dir_all(&bgd).unwrap();
        std::fs::write(bgd.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
        std::fs::write(bgd.join("commondir"), "../..\n").unwrap();
        let bwt = d.path().join("feat");
        std::fs::create_dir_all(&bwt).unwrap();
        std::fs::write(bwt.join(".git"), format!("gitdir: {}\n", bgd.display())).unwrap();
        assert_eq!(
            repo_and_branch(&bwt),
            (Some("api".into()), Some("feat".into()))
        );
        assert_eq!(
            repo_and_branch(Path::new("")),
            (None, None),
            "no working directory"
        );
        let outside = tempfile::tempdir().unwrap();
        // A temporary directory may sit inside a repository on some
        // machines; only assert there is no branch from a file there.
        let (_, b) = repo_and_branch(outside.path());
        assert!(b.as_deref() != Some("fix"));
    }
}
