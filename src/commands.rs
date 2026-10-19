//! The actions offered by the command palette. Pure data and filtering, so it is testable
//! without any UI.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Send,
    Save,
    NewTab,
    CloseTab,
    CopyCurl,
    BeautifyBody,
    CycleTheme,
    ToggleSplit,
    ManageEnvironments,
    ImportExport,
}

#[derive(Debug)]
pub struct Entry {
    pub command: Command,
    pub title: &'static str,
    /// Shortcut key shown next to the entry (with the platform modifier prepended), if any.
    pub shortcut: Option<&'static str>,
}

pub const ENTRIES: [Entry; 10] = [
    Entry {
        command: Command::Send,
        title: "Send request",
        shortcut: Some("Enter"),
    },
    Entry {
        command: Command::Save,
        title: "Save request",
        shortcut: Some("S"),
    },
    Entry {
        command: Command::NewTab,
        title: "New tab",
        shortcut: Some("T"),
    },
    Entry {
        command: Command::CloseTab,
        title: "Close tab",
        shortcut: Some("W"),
    },
    Entry {
        command: Command::CopyCurl,
        title: "Copy request as cURL",
        shortcut: None,
    },
    Entry {
        command: Command::BeautifyBody,
        title: "Beautify JSON body",
        shortcut: None,
    },
    Entry {
        command: Command::CycleTheme,
        title: "Change theme (system / light / dark)",
        shortcut: None,
    },
    Entry {
        command: Command::ToggleSplit,
        title: "Toggle response position (below / beside)",
        shortcut: None,
    },
    Entry {
        command: Command::ManageEnvironments,
        title: "Manage environments",
        shortcut: None,
    },
    Entry {
        command: Command::ImportExport,
        title: "Import / export collections",
        shortcut: None,
    },
];

/// Entries whose title contains every whitespace-separated word of `query` (any case).
/// Titles that start with the first word come first; otherwise the list order is kept.
pub fn filter(query: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<&'static Entry> = ENTRIES
        .iter()
        .filter(|e| {
            let title = e.title.to_lowercase();
            words.iter().all(|w| title.contains(w.as_str()))
        })
        .collect();
    if let Some(first) = words.first() {
        hits.sort_by_key(|e| !e.title.to_lowercase().starts_with(first.as_str()));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_lists_everything_in_order() {
        let all = filter("  ");
        assert_eq!(all.len(), ENTRIES.len());
        assert_eq!(all[0].command, Command::Send);
    }

    #[test]
    fn matches_all_words_in_any_order_and_case() {
        let hits: Vec<_> = filter("CURL copy").iter().map(|e| e.command).collect();
        assert_eq!(hits, vec![Command::CopyCurl]);
        assert!(filter("zzz").is_empty());
    }

    #[test]
    fn prefix_matches_rank_first() {
        // "tab" is contained in "New tab" and "Close tab"; neither starts with it, order kept.
        let hits: Vec<_> = filter("tab").iter().map(|e| e.command).collect();
        assert_eq!(hits, vec![Command::NewTab, Command::CloseTab]);
        // "toggle" starts one title; "theme" is not a prefix of anything but "Change theme" matches.
        assert_eq!(filter("toggle")[0].command, Command::ToggleSplit);
        // "s" prefixes "Send" and "Save" -> they come before other titles containing an s.
        let top: Vec<_> = filter("s").iter().take(2).map(|e| e.command).collect();
        assert_eq!(top, vec![Command::Send, Command::Save]);
    }
}
