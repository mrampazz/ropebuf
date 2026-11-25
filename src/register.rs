use arboard::Clipboard;
use std::collections::HashMap;
use std::collections::VecDeque;

pub struct RegisterManager {
    pub unnamed: String,
    pub history: VecDeque<String>,
    pub named: HashMap<char, String>,
    pub clipboard: Option<Clipboard>,
}

impl RegisterManager {
    pub fn new() -> Self {
        let clipboard = Clipboard::new().ok();
        Self {
            unnamed: String::new(),
            history: VecDeque::with_capacity(10),
            named: HashMap::new(),
            clipboard,
        }
    }

    pub fn push_yank(&mut self, text: String) {
        if text.is_empty() {
            return;
        }

        self.unnamed = text.clone();

        self.history.push_front(text);
        if self.history.len() > 10 {
            self.history.pop_back();
        }
    }

    pub fn set(&mut self, reg: char, text: String) {
        if text.is_empty() {
            return;
        }

        match reg {
            '+' => {
                if let Some(cb) = &mut self.clipboard {
                    let _ = cb.set_text(text.clone());
                }
                self.unnamed = text;
            }
            '"' => {
                self.unnamed = text.clone();
                self.history.push_front(text);
                if self.history.len() > 10 {
                    self.history.pop_back();
                }
            }
            '0'..='9' => {
                // Numbered registers are typically read-only history, but handled if needed
                // TODO: do we need to do anything here?
            }
            _ => {
                self.named.insert(reg, text);
            }
        }
    }

    pub fn set_named(&mut self, name: char, text: String) {
        self.named.insert(name, text);
    }

    pub fn get(&mut self, reg: char) -> Option<String> {
        match reg {
            '+' => {
                if let Some(cb) = &mut self.clipboard {
                    cb.get_text().ok()
                } else {
                    Some(self.unnamed.clone())
                }
            }
            '"' => Some(self.unnamed.clone()),
            '0'..='9' => {
                let idx = reg.to_digit(10)? as usize;
                self.history.get(idx).cloned()
            }
            _ => self.named.get(&reg).cloned(),
        }
    }
}
