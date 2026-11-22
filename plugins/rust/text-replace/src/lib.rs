//! Text Replace: a `compose` plugin, after Vencord's `TextReplace`.
//!
//! You keep a list of find/replace rules in its settings panel. Each rule is a
//! plain string or a regex (`$1`-style capture references in the
//! replacement). Rules run in order on every message *you* send, just before
//! it goes out: the host hands this plugin your draft through `on-compose`
//! and sends whatever comes back. It never sees anyone else's messages, and
//! it can't send anything on its own.

use regex_lite::Regex;
use rekindle_sdk::prelude::*;

const PANEL: &str = "settings";
const MAX_RULES: usize = 32;
const KEY: &str = "rules";
/// Field and record separators for storage (stripped from typed values).
const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

#[derive(Clone, Debug, Default, PartialEq)]
struct Rule {
    find: String,
    replace: String,
    regex: bool,
}

/// A rule plus, for a regex rule, its compiled pattern or compile error.
struct Compiled {
    rule: Rule,
    re: Option<Result<Regex, String>>,
}

impl Compiled {
    fn new(rule: Rule) -> Self {
        let re = (rule.regex && !rule.find.is_empty())
            .then(|| Regex::new(&rule.find).map_err(|e| e.to_string()));
        Self { rule, re }
    }

    fn apply(&self, text: &str) -> String {
        if self.rule.find.is_empty() {
            return text.to_string();
        }
        match &self.re {
            Some(Ok(re)) => re
                .replace_all(text, self.rule.replace.as_str())
                .into_owned(),
            Some(Err(_)) => text.to_string(),
            None => text.replace(&self.rule.find, &self.rule.replace),
        }
    }

    fn error(&self) -> Option<&str> {
        match &self.re {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }
}

struct TextReplace {
    rules: Vec<Compiled>,
}

fn decode(s: &str) -> Vec<Rule> {
    s.split(RS)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let mut f = r.split(FS);
            Some(Rule {
                find: f.next()?.into(),
                replace: f.next()?.into(),
                regex: f.next()? == "1",
            })
        })
        .take(MAX_RULES)
        .collect()
}

fn encode(rules: &[Rule]) -> String {
    rules
        .iter()
        .map(|r| {
            format!(
                "{}{FS}{}{FS}{}",
                r.find,
                r.replace,
                if r.regex { "1" } else { "0" }
            )
        })
        .collect::<Vec<_>>()
        .join(&RS.to_string())
}

/// Strip the separators so a typed value can't corrupt storage.
fn clean(s: &str) -> String {
    s.chars().filter(|&c| c != FS && c != RS).collect()
}

impl TextReplace {
    fn save(&self) {
        let rules: Vec<Rule> = self.rules.iter().map(|c| c.rule.clone()).collect();
        if let Err(e) = storage::set(KEY, encode(&rules)) {
            ui::toast_kind(&format!("Couldn't save rules: {e:?}"), ToastKind::Error);
        }
    }

    fn show(&self) {
        let mut p = Panel::new(PANEL, "Text Replace")
            .text("Rules run top to bottom on messages you send. Regex rules use $1, $2, ... for capture groups.");
        for (i, c) in self.rules.iter().enumerate() {
            p = p
                .heading(&format!("Rule {}", i + 1))
                .text_input(
                    &format!("find-{i}"),
                    "Find",
                    &c.rule.find,
                    "text or pattern",
                )
                .text_input(
                    &format!("replace-{i}"),
                    "Replace",
                    &c.rule.replace,
                    "replacement",
                )
                .toggle(&format!("regex-{i}"), "Regex", c.rule.regex);
            if let Some(e) = c.error() {
                p = p.text(&format!("Invalid regex, rule skipped: {e}"));
            }
            p = p.button(&format!("delete-{i}"), "Delete rule");
        }
        if self.rules.len() < MAX_RULES {
            p = p.separator().button("add", "Add rule");
        }
        p.show();
    }

    fn set(&mut self, i: usize, f: impl FnOnce(&mut Rule)) {
        let mut rule = self.rules[i].rule.clone();
        f(&mut rule);
        self.rules[i] = Compiled::new(rule);
    }
}

impl Plugin for TextReplace {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let rules = decode(&storage::get(KEY).unwrap_or_default())
            .into_iter()
            .map(Compiled::new)
            .collect();
        let me = Self { rules };
        me.show();
        Ok(me)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != PANEL {
            return;
        }
        if ev.element == "add" {
            if self.rules.len() < MAX_RULES {
                self.rules.push(Compiled::new(Rule::default()));
            }
        } else {
            let Some((field, i)) = ev.element.rsplit_once('-') else {
                return;
            };
            let Some(i) = i.parse::<usize>().ok().filter(|&i| i < self.rules.len()) else {
                return;
            };
            let was_valid = self.rules[i].error().is_none();
            match (field, ev.value) {
                ("find", UiValue::Text(t)) => self.set(i, |r| r.find = clean(&t)),
                ("replace", UiValue::Text(t)) => self.set(i, |r| r.replace = clean(&t)),
                ("regex", UiValue::Toggled(on)) => self.set(i, |r| r.regex = on),
                ("delete", UiValue::Clicked) => {
                    self.rules.remove(i);
                }
                _ => return,
            }
            // Typing into a field shouldn't rebuild the panel under the
            // cursor, unless the rule's regex just became (in)valid.
            let structural = field == "delete" || field == "regex";
            if !structural && self.rules[i].error().is_none() == was_valid {
                self.save();
                return;
            }
        }
        self.save();
        self.show();
    }

    fn on_compose(&mut self, draft: &Draft) -> ComposeResult {
        let out = self
            .rules
            .iter()
            .fold(draft.text.clone(), |t, c| c.apply(&t));
        if out == draft.text {
            ComposeResult::Keep
        } else {
            compose::replace(out)
        }
    }
}

rekindle_sdk::plugin!(TextReplace);
