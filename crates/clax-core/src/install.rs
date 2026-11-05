//! A queue's installs: one record per site that embeds the overlay and
//! files into the queue, with the settings that site's overlay reads
//! (its white-label wording, whether it shows the Report button and the
//! element picker, and how the page opens it: a trigger selector, the `F`
//! key, a ribbon). The owner edits them in Clax (`clax_store::queues`);
//! the host page carries only the install's ID and fetches the rest from
//! `GET /api/installs/<id>/config`.

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// The longest install name, in characters.
pub const MAX_INSTALL_NAME_CHARS: usize = 120;
/// The longest piece of wording (product name, labels), in characters.
pub const MAX_WORDING_CHARS: usize = 80;
/// The most installs a queue holds.
pub const MAX_QUEUE_INSTALLS: usize = 64;
/// The longest trigger selector, in characters.
pub const MAX_TRIGGER_CHARS: usize = 256;
/// The longest ribbon wording, in characters: what fits along the narrower
/// band (web/overlay/src/ribbon.ts) at its smallest lettering.
pub const MAX_RIBBON_CHARS: usize = 14;

/// What an install's overlay is configured with. Wording left out is the
/// overlay's own default; it never names a product (or "Clax") unless
/// `product_name` does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallConfig {
    /// The name the overlay calls itself by; `None`: no product name.
    #[serde(default)]
    pub product_name: Option<String>,
    /// The Report button's label; `None`: the overlay's "Report".
    #[serde(default)]
    pub report_label: Option<String>,
    /// The words before the queue's name in the Report panel; `None`: the
    /// overlay's "Files into".
    #[serde(default)]
    pub queue_label: Option<String>,
    /// Whether the overlay shows "Your reports" beside Report (the page's
    /// `<meta name="clax-reports">` `button`; false is its `off`).
    #[serde(default = "yes")]
    pub reports_button: bool,
    /// Whether the overlay offers its element picker.
    #[serde(default = "yes")]
    pub picker: bool,
    /// The overlay's accent, `#rrggbb`; `None`: its own.
    #[serde(default)]
    pub brand_color: Option<String>,
    /// The CSS selector of the page's element that opens the overlay, as a
    /// tag manager's trigger names one; `None`: the page's own option, and
    /// the corner launcher. With one, the overlay shows no corner launcher.
    #[serde(default)]
    pub trigger: Option<String>,
    /// Whether the `F` key opens the overlay (never in a text field).
    #[serde(default)]
    pub f_key: bool,
    /// Whether the overlay draws its ribbon across the page's top-left
    /// corner. It needs `ribbon_text`.
    #[serde(default)]
    pub ribbon: bool,
    /// What the ribbon says, such as "Early Access". The band shows it in
    /// capitals; a screen reader hears it as written.
    #[serde(default)]
    pub ribbon_text: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for InstallConfig {
    fn default() -> Self {
        InstallConfig {
            product_name: None,
            report_label: None,
            queue_label: None,
            reports_button: true,
            picker: true,
            brand_color: None,
            trigger: None,
            f_key: false,
            ribbon: false,
            ribbon_text: None,
        }
    }
}

/// An install as its queue's members read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Install {
    pub id: String,
    pub queue_id: String,
    pub name: String,
    /// The site's origins (normalized by `queue::normalize_origin`): the
    /// config answers these alone across origins.
    pub origins: Vec<String>,
    pub config: InstallConfig,
    pub created_at: String,
    pub updated_at: String,
}

fn invalid(msg: impl Into<String>) -> CoreError {
    CoreError::invalid("invalid_install", msg)
}

/// `name` trimmed: 400 `invalid_install` when blank or over
/// [`MAX_INSTALL_NAME_CHARS`].
///
/// # Errors
/// As above.
pub fn check_install_name(name: &str) -> Result<String> {
    let t = name.trim();
    if t.is_empty() || t.chars().count() > MAX_INSTALL_NAME_CHARS {
        return Err(invalid(format!(
            "an install's name is 1 to {MAX_INSTALL_NAME_CHARS} characters"
        )));
    }
    Ok(t.to_string())
}

fn wording(field: &str, w: Option<String>) -> Result<Option<String>> {
    let Some(w) = w else { return Ok(None) };
    let t = w.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > MAX_WORDING_CHARS || t.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{field} is at most {MAX_WORDING_CHARS} characters, on one line"
        )));
    }
    Ok(Some(t.to_string()))
}

/// `t` trimmed (blank is `None`). Only a browser can parse a selector, and
/// a page's overlay ignores one it cannot; this refuses what never is one.
fn trigger(t: Option<String>) -> Result<Option<String>> {
    let Some(t) = t else { return Ok(None) };
    let s = t.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > MAX_TRIGGER_CHARS
        || s.chars().any(|c| c.is_control() || "{}<".contains(c))
    {
        return Err(invalid(format!(
            "the trigger is one CSS selector of at most {MAX_TRIGGER_CHARS} characters"
        )));
    }
    Ok(Some(s.to_string()))
}

/// `c` with its wording and trigger trimmed (blank is `None`) and its
/// colour lowercased.
///
/// # Errors
/// 400 `invalid_install` for wording over [`MAX_WORDING_CHARS`] or with
/// control characters, a `brand_color` that is not `#rrggbb`, a `trigger`
/// that is no selector, or a `ribbon` with no `ribbon_text` or one over
/// [`MAX_RIBBON_CHARS`].
pub fn check_install_config(c: InstallConfig) -> Result<InstallConfig> {
    let ribbon_text = wording("the ribbon's wording", c.ribbon_text)?;
    if ribbon_text
        .as_deref()
        .is_some_and(|t| t.chars().count() > MAX_RIBBON_CHARS)
    {
        return Err(invalid(format!(
            "the ribbon's wording is at most {MAX_RIBBON_CHARS} characters"
        )));
    }
    if c.ribbon && ribbon_text.is_none() {
        return Err(invalid("the ribbon needs its wording (ribbon_text)"));
    }
    let brand_color = match c.brand_color.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s) => {
            let hex = s.strip_prefix('#').unwrap_or("");
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid("the accent colour (brand_color) is #rrggbb"));
            }
            Some(s.to_ascii_lowercase())
        }
    };
    Ok(InstallConfig {
        product_name: wording("the product name", c.product_name)?,
        report_label: wording("the Report button's label", c.report_label)?,
        queue_label: wording("the words before the queue's name", c.queue_label)?,
        reports_button: c.reports_button,
        picker: c.picker,
        brand_color,
        trigger: trigger(c.trigger)?,
        f_key: c.f_key,
        ribbon: c.ribbon,
        ribbon_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_left_out_is_the_overlays_defaults() {
        let c: InstallConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, InstallConfig::default());
        assert!(c.reports_button && c.picker);
        assert_eq!(c.product_name, None);
        // An install stored before these fields opens feedback as it did.
        assert_eq!(c.trigger, None);
        assert!(!c.f_key && !c.ribbon);
        assert_eq!(c.ribbon_text, None);
    }

    #[test]
    fn how_a_site_opens_feedback_is_checked() {
        let c = check_install_config(InstallConfig {
            trigger: Some("  .feedback-btn ".into()),
            f_key: true,
            ribbon: true,
            ribbon_text: Some(" Early Access ".into()),
            ..InstallConfig::default()
        })
        .unwrap();
        assert_eq!(c.trigger.as_deref(), Some(".feedback-btn"));
        assert_eq!(c.ribbon_text.as_deref(), Some("Early Access"));
        assert!(c.f_key && c.ribbon);

        let blank = check_install_config(InstallConfig {
            trigger: Some("   ".into()),
            ribbon_text: Some("  ".into()),
            ..InstallConfig::default()
        })
        .unwrap();
        assert_eq!((blank.trigger, blank.ribbon_text), (None, None));

        for bad in [
            InstallConfig {
                ribbon: true,
                ..InstallConfig::default()
            },
            InstallConfig {
                ribbon: true,
                ribbon_text: Some("x".repeat(MAX_RIBBON_CHARS + 1)),
                ..InstallConfig::default()
            },
            InstallConfig {
                trigger: Some("x".repeat(MAX_TRIGGER_CHARS + 1)),
                ..InstallConfig::default()
            },
            InstallConfig {
                trigger: Some("a{color:red}".into()),
                ..InstallConfig::default()
            },
            InstallConfig {
                trigger: Some("<button>".into()),
                ..InstallConfig::default()
            },
        ] {
            assert!(check_install_config(bad).is_err());
        }
    }

    #[test]
    fn wording_is_trimmed_and_blank_wording_is_none() {
        let c = check_install_config(InstallConfig {
            product_name: Some("  Acme Feedback ".into()),
            report_label: Some("   ".into()),
            brand_color: Some("#E0532F".into()),
            ..InstallConfig::default()
        })
        .unwrap();
        assert_eq!(c.product_name.as_deref(), Some("Acme Feedback"));
        assert_eq!(c.report_label, None);
        assert_eq!(c.brand_color.as_deref(), Some("#e0532f"));
    }

    #[test]
    fn bad_config_is_refused() {
        for bad in [
            InstallConfig {
                brand_color: Some("red".into()),
                ..InstallConfig::default()
            },
            InstallConfig {
                queue_label: Some("x".repeat(MAX_WORDING_CHARS + 1)),
                ..InstallConfig::default()
            },
            InstallConfig {
                report_label: Some("two\nlines".into()),
                ..InstallConfig::default()
            },
        ] {
            assert!(check_install_config(bad).is_err());
        }
        assert!(serde_json::from_str::<InstallConfig>(r##"{"colour":"#fff"}"##).is_err());
        assert!(check_install_name(" ").is_err());
        assert_eq!(check_install_name(" Changelist ").unwrap(), "Changelist");
    }
}
