# Every view, one by one

`thc` with no arguments opens the app. Seven views sit along the top. Press `1`–`7`, or click a
tab. The bottom row always shows the keys that work right now. `F1` (or `?` outside writing)
shows all of them.

[render: the header with seven tabs, Today active]

## 1 · Today

**What needs you now**, from every page and day:
- **Overdue** (red)
- **Today**: due, scheduled, or a reminder firing
- **Doing**
- **Next 7 days**
- **Done today**

| Do | Key |
|---|---|
| Mark done | `x` |
| Change the due / scheduled date | `d` / `s` |
| Set the priority | `p`, then `h` `m` `l` |
| Tag | `#` |
| Move it somewhere | `m` |
| Open it where it lives | `Enter` |
| Today ⇄ agenda (a week by day) | `w` |
| Snooze or acknowledge a reminder | `z` / `Z` |

When a reminder fires while you're in `thc`, a toast offers `x done  z snooze  Z ack`.

## 2 · Inbox

Things captured without a home: from `thc add --inbox`, from agents, or from the `drop/`
folder. Triage each one with `m` (move to a page or day), `t` (make it a task), `d` (give it
a date), `x` (done) or `D` (delete). The cursor moves on after each, so triage is fast.

## 3 · Tasks

Every open task, filtered by a query. Press `f` and type, for example
`status:open #work due<=+7d sort:due`. Saved filters sit under `1`–`9`. At 120 columns the list
becomes a table with due, scheduled, priority, where and who. Press `,` to change the sort.

[render: Tasks with the filter row and the saved row]

## 4 · Pages

Your pages, with how many open items each has. Type to find one. `Enter` opens it in the editor.
Pages are made by linking (`[[Name]]`) or from the finder (`+ new page`).

`4` (or the Pages tab) always shows this list, never a page. The cursor rests on the page you
opened last, so `4 Enter` reopens it, and `Esc` in a page comes back here.

In a terminal 120 columns or wider, a page has a **rail** of your pages on its left (recent
first, then A–Z, with open counts). Click one to switch, or `PAGES` for this list. A journal
day gets a rail of days. Narrower, a crumb (`¶ Pages › Health`) sits above the title instead,
with `Pages` clickable. Focus hides both (`:focus` element `nav` brings them back), and
`page_rail = false` under `[tui]` in your config turns the rail off.

## 5 · Journal

One page per day, opened as a document you write in (see [Writing](writing.md)). The strip
under the date jumps between days (`⌃P` / `⌃N`, or click a day). **Also today** at the bottom
shows what's due today from other pages.

[render: a journal day with prose, tasks, and also today]

## 6 · Search

Full text across pages, tags and notes, as you type. `Enter` opens the match in context. `⌃Q`
turns the search into a Tasks filter.

## 7 · Log

Every change, newest first: who made it (`you`, `◆ claude`), when, and what. `u` undoes the
selected change (as a new change: nothing is erased). `r` switches to the **review lane**, where
agents' changes wait for you: `a` accept, `u` undo, `A` accept all.

## Around the edges

- **The detail pane** (`\`, at 120+ columns): everything about the selected item, plus its
  history and links.
- **Contexts** (`C`): show only what matches a saved view, e.g. only `#work` while at work.
  It's always visible in the header when on, and it's never hidden.
- **Focus** (`⌥Z` while writing): hides everything but your words. Choose what stays with
  `:focus`.
- **The leader** (`Space`, in any list): a panel shows every command, grouped by what it does.
  `space g` go (`g t` Today, `g j` Journal, `g d` a date…), `space f` find, `space n` new,
  `space t` toggle (focus, context, detail pane…), `space v` saved views. You never need to
  remember a key: press `Space` and read. `⌫` steps back, and `Esc` closes. While writing, Space
  is just a space.
- **Help** (`F1`, or `?` in lists): the keys for exactly where you are, grouped, scrolling when
  it doesn't fit. `??` (or `space ?`) shows every key everywhere.
- **The palette** (`:`, or `space space`): every command by name, with its key. Type a few
  letters and press Enter.
- **`thc keys`** prints every key from the command line. To change keys, see
  [Keys › Remapping](keys.md#remapping).
