//! The vault the flows start from: a person's few pages, a journal day, tasks, and long pages
//! for scrolling and the budgets. Built through the writer, as `thc` would, on a scratch vault.

use thc_core::builder::TxBuilder;
use thc_core::vault::Vault;

/// What a flow's vault holds besides the base pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    /// The base pages only.
    #[default]
    Small,
    /// Plus "Long Page" (300 lines).
    Long,
    /// Plus "Huge Page" (5,000 lines).
    Huge,
}

/// A 300-character paragraph that soft-wraps on any screen.
pub const LONG_PARA: &str = "This paragraph is long on purpose so that it wraps across several rows of the editor at every width we test, and typing inside it has to keep every row above the caret exactly where it was, with nothing jumping or shifting while the words flow on to the next row and beyond.";

pub fn seed(vault: &mut Vault, size: Size) {
    let today = thc_core::dates::today();
    let yesterday = today.pred_opt().unwrap();
    let cap = |t: &str| thc_core::capture::parse(t, today).unwrap();
    vault
        .transact(|st| {
            let mut b = TxBuilder::new(st, today);
            // Pages, each with an outline (made first: a link to a missing page makes a stub).
            let q4 = b.create_page("Q4 Plan", &[])?;
            let garden = b.create_page("Garden", &[])?;
            let lisbon = b.create_page("Lisbon flat", &[])?;
            let goals = b.create_from_capture(Some(q4.clone()), &cap("Goals for the quarter"), None)?;
            b.create_from_capture(Some(goals.clone()), &cap("Grow the newsletter"), None)?;
            b.create_from_capture(Some(goals), &cap("Ship the [[Garden]] redesign"), None)?;
            b.create_from_capture(Some(q4.clone()), &cap("[ ] Draft the budget due:+3d"), None)?;
            b.create_from_capture(Some(q4.clone()), &cap("Notes from the offsite with [[Lisbon flat]] ideas"), None)?;
            b.create_from_capture(Some(q4.clone()), &cap(LONG_PARA), None)?;
            b.create_from_capture(Some(q4), &cap("Last line of the plan"), None)?;

            b.create_from_capture(Some(garden.clone()), &cap("Tomatoes need staking"), None)?;
            b.create_from_capture(Some(garden.clone()), &cap("Compost by the fence"), None)?;
            b.create_from_capture(Some(garden), &cap("See [[Q4 Plan]] for the budget"), None)?;

            b.create_from_capture(Some(lisbon.clone()), &cap("Two bedrooms near the river"), None)?;
            b.create_from_capture(Some(lisbon), &cap("Ask about the lease 日本語 and 🙂"), None)?;

            // Today's journal and yesterday's.
            let j = b.journal(today)?;
            b.create_from_capture(Some(j.clone()), &cap("Morning notes"), None)?;
            b.create_from_capture(Some(j.clone()), &cap("[ ] call the bank !high"), None)?;
            b.create_from_capture(Some(j.clone()), &cap("read [[Q4 Plan]] before standup"), None)?;
            b.create_from_capture(Some(j), &cap("[ ] water the plants #home"), None)?;
            let y = b.journal(yesterday)?;
            b.create_from_capture(Some(y), &cap("Yesterday's entry"), None)?;

            // Tasks elsewhere: the inbox.
            b.create_from_capture(None, &cap("[ ] write the plan due:today #work"), None)?;
            b.create_from_capture(None, &cap("[ ] book flights due:+2d"), None)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    let lines = match size {
        Size::Small => 0,
        Size::Long => 300,
        Size::Huge => 5000,
    };
    if lines > 0 {
        let title = if size == Size::Long { "Long Page" } else { "Huge Page" };
        vault
            .transact(|st| {
                let mut b = TxBuilder::new(st, today);
                let p = b.create_page(title, &[])?;
                for i in 0..lines {
                    let t = match i % 10 {
                        3 => format!("Line {i}: {}", &LONG_PARA[..120]),
                        7 => format!("[ ] Line {i}: a task to do"),
                        9 => format!("Line {i}: links to [[Garden]]"),
                        _ => format!("Line {i}: plain words in a line"),
                    };
                    b.create_from_capture(Some(p.clone()), &cap(&t), None)?;
                }
                Ok((b.finish(), ()))
            })
            .unwrap();
    }
}
