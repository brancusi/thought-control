**0.9.59**

- **`thc team add` grows a running team.** `thc team add engineer --agent codex` opens one more agent in the team's tab, briefed for its role on the project's board; `--task <id>` hands it a task to claim first. `thc team rm` takes one out. Every add and remove is noted on the board.
- **Under the hood:** thc's commands each live in their own file now, so several agents can add commands at once without colliding.

**0.9.58**

- **`thc team up` starts a team of agents.** `thc team up pm engineer designer --agent claude` opens one agent per role in new panes (herdr first, then WezTerm; elsewhere it prints the commands to run), each briefed to work from the project's board. `thc team ls` shows who's there and what they're on; `thc team down` closes only the panes it opened.
- **`thc next` and big queries are faster.** Finding the next ready task no longer slows down in a large vault.

**0.9.57**

- **See what changed, right in thc.** Click the version at the bottom right (or type `:about`) for an About page: every release since you last looked, highlighted, then this thc's version, vault, background service and terminal, then the full changelog, which you can search with `/`. After an update, the version reads `· new` until you've had a look.
- **Agents get roles and messages.** `thc prime --role engineer` briefs an agent for its role on the project's board, and `thc next --role` picks work routed to it. `thc msg <role> "…"` leaves a message for a teammate, and `is:unread` finds what's waiting for you.
- **`thc shot` puts a screenshot in your notes.** Pick an area of the screen and it lands in today's journal (or `--under` a note). Attached pictures are tidied as they come in: location data removed, turned the right way up, and very large ones scaled down. `thc attach` takes several files at once.
- **`thc watch --for <role>`** lets an agent wait for its messages and newly ready tasks instead of polling.
- **Views sorted by `sort:order` keep the page's order** in every section.

**0.9.56**

- **thc can be your agents' task board.** Tell an agent to "coordinate with thc" and it knows the routine: `thc next` gives it the top ready task nobody has taken (in the order you put them on the page, skipping anything blocked), it claims it so no one else does, writes its progress as notes, and leaves the finished task for you to review. The instructions come with thc, so agents in any project pick them up after this update.
- **`sort:order` in queries** lists notes in the order they appear on the page, parents before their children.

**0.9.55**

- **⌃T twice never undoes itself.** Making a line a task and then ticking it off quickly could come back as an open task, because the first save's answer landed on top of the second change. Saves now go one at a time, and an answer never overwrites anything you've changed since. The same fix covers typing, undo and leaving a page while a save is still on its way.
- **A day opens instantly again.** Opening a day or the TUI read through your whole history first, so it got slower as your vault grew (55 ms in a large vault). It's back to about 4 ms.

**0.9.54**

- **Typing in a long page is instant again.** Since 0.9.49, each key in a page of thousands of lines took about ten times longer than it should (60 ms a key at 5,000 lines). It's back to a few milliseconds, however long the page.

**0.9.53**

- **Attachments are things you can find.** Every image or file in your notes is now also an item of its own, the same one wherever it appears and on every device. `thc q is:image` lists your images, `thc show <id>` says which notes use one, and `thc attach` records a picture's size. Your notes' text doesn't change at all. The first time thc writes after updating, it sets this up for the attachments you already have, in one step you can undo (`thc undo`).
- **The background service answers right away** when it starts, even if your Mac is slow to start watching files. Before, the TUI could wait up to half a minute for it.

**0.9.52**

- **`thc keys --edit` makes remapping easy to find.** It opens your config at a list of every key, commented out and grouped by where it works. Uncomment a line, change the key, save, and thc checks it: `keys ok · 1 remapped`, or the problem and its line. Run it again any time: the list refreshes and your changes stay.
- **`?` on a view counts what the list shows,** across all your vaults, and long queries now wrap instead of being cut off.

**0.9.51**

- **Lists stay in order.** Clicking, opening and coming back no longer reshuffles anything. The page list beside a document keeps its order while you move between pages. When you finish a task it stays where it is, dimmed, until you come back to the view (or press ⌃L). A note added by an agent appears without moving your selection.
- **Today shows where each line comes from.** With more than one vault, every line ends with its vault's name in its colour. Press `*` (or click the scope in the header) to choose which vaults Today shows on this device. `s` in that picker makes it the default everywhere.
- **`?` explains a view.** On Today, Inbox, Tasks or a saved view, `?` shows how it's built: each section with how many it holds, its query, and what that means with dates filled in. Press `e` to edit it in your editor, `c` to copy it as a new view, or `r` to reset a built-in one. Press `?` again for the keys.

**0.9.50**

- **Back and forward, like a browser.** ⌘[ goes back to where you were and ⌘] forward again, across pages, days, views and vaults, with the cursor where you left it. In any terminal, ⌃⌥← and ⌃⌥→ do the same, and `:history` (or space g h) lists the places you've been. Click one to go there.
- **Esc still goes "up":** after following links, Esc takes you back to where you started (the list, or the day). ⌘[ goes to the page before.
- In WezTerm, ⌘[ and ⌘] work once thc has run once after updating. It refreshes its own key settings and leaves your wezterm.lua alone.

**0.9.49**

- **⌃T works on the line you're on.** In a paragraph of several lines, ⌃T makes just that line a task. The lines above and below stay text, and ⌃T back joins them up again. ⌘Z undoes it in one step.
- **Text never jumps when you change a line's kind.** A blank line you typed stays where it is when a line becomes a task, goes back to text, or is nested with Tab. It's saved with the note, and `thc edit`, `thc import` and the Markdown export keep it too.

**0.9.48**

- **Every popup works with the mouse.** In the vault picker, the command palette, the go-to finder, move, the `[[` suggestions, help, `:focus`, compare and the capture bar: hovering a row highlights it, one click picks it, the wheel scrolls, and a click outside closes. The vault picker has a `+ new vault` row.
- **Clicking a row in a list selects that row.** In the Log especially, a click could highlight a different entry and jump around.
- **Vaults show up live.** A vault made, renamed or removed elsewhere (say, by an agent running `thc vault new`) appears in an open vault picker and in Today and Agenda without reopening anything. The background service picks up a new vault at once instead of within half a minute.
- **A thc whose terminal window went away now quits** (saving what you typed) instead of running on in the background at full CPU, and `kill` always ends thc within a couple of seconds. Anything it couldn't save waits in crash recovery.
- **For agents:** rule 1 now asks agents to use their actual name in `THC_ACTOR`, and to record a session id as a `session_id` property. Claiming a task with `thc set <id> status=doing owner=<you> --expect status=todo` is proven to have exactly one winner when agents race, and `thc add --plain` keeps `[[…]]` as text.

**0.9.47**

- **A click on a link goes there**, saving first, and **Esc** comes back to where you were. To edit a link instead, click its `[[` or `]]`, ⌥-click it, or arrow into it (then `⌃O` goes there). A drag that starts on a link still selects.
- **Hovering a link underlines it**, so you can see it's clickable.
- Clicking a `#tag` still just places the cursor.

**0.9.46**

- **The footer shows thc's version** at the right, when there's room.
- **On a Mac, ⌘ keys come first** in the key help (F1): `⌘C ⌃C copy`, `⌘V ⌃V paste`, `⌘Z ⌃Z undo`. If thc hasn't seen your terminal send ⌘ keys yet, help says how to turn them on (`thc setup wezterm`).
- **Fixed in editing** (found by a new suite of editing tests):
  - ⌘Z brings back the selection you had before the change, not just the text.
  - Typing over a selection, then more letters, is a single ⌘Z.
  - ⌃K and ⌃U with a selection delete only the selection.
  - ⌥⌫ then ⌘Z puts the cursor back where it was.
  - Copying across notes no longer adds a stray `- ` to the first line.
  - ⌘C says what it copied ("copied 14 chars", "copied 3 notes"), or "nothing selected".
- `thc setup wezterm` lists ⌘C ⌘X ⌘V ⌘A ⌘Z ⇧⌘Z in its summary.

**0.9.45: editing like a Mac text field**

- **⌘C, ⌘X, ⌘A, ⌘Z and ⇧⌘Z work in thc** (in WezTerm, run `thc setup wezterm --yes` again to get them). ⌘C copies your thc selection as Markdown. A selection you made in WezTerm itself, with an ⌥-drag, is still copied the WezTerm way.
- **← and → with a selection** go to its start or end and stop there, as on a Mac. They no longer jump to the line before or after.
- **Moving around an image never opens it.** A click puts the cursor there, and Enter starts a new line after it. ⌃O or a double-click opens it.

**0.9.44**

- A date thc can't read (`due:fryday`, `--due fryday`) now exits with code 6 (fix your input), as the docs say, instead of 1. The message is unchanged: `can't read "fryday" as a date · try fri, +3d or 2026-10-09`.

**0.9.43: WezTerm keys in one command**

- **`thc setup wezterm --yes`** sets up the Mac editing keys and ⌘V screenshots. It writes thc's own key file and adds one marked block to the config WezTerm actually loads (`~/.wezterm.lua` or `~/.config/wezterm/wezterm.lua`). It backs that file up first, and never adds the block twice. `thc setup wezterm --undo --yes` takes both out again.
- **The keys only act inside thc.** In your shell, an editor or SSH, ⌘←, ⌥←, ⌘⌫ and ⌘V do exactly what they did before.
- It also turns on WezTerm's kitty keyboard mode for every program, which is what lets ⇧Enter make a line break in thc.
- `thc setup`, run in WezTerm, offers this once. It never changes your config without a yes.

**0.9.42: screenshots the usual way**

- **Drag a file onto thc** and it's attached right away, with no question. The bar says `attached wezterm · ⌃Z keep the path`: ⌃Z turns it back into the file's path as text, and ⌃Z again removes that. ⌥V before a drag pastes the path instead.
- **⌘V pastes a screenshot.** Copy one (⌃⇧⌘4), then ⌘V while writing: it's attached as its own line, and ⌃Z undoes it. Text pastes exactly as before.
  - In WezTerm, run `thc setup wezterm --yes` again so ⌘V reaches thc. It only does that while thc is running in the pane; everything else, and SSH, paste as usual.
  - **⌃V** does the same in any terminal, and ⌥V still works too.

**0.9.41: screenshots show in WezTerm**

- **An attached image draws under its chip** in WezTerm and iTerm2, and in kitty and Ghostty. It's sized to the text column and at most 16 lines tall. The space is kept while you type, so nothing jumps.
- Over SSH, and with `[tui] images = "chips"` in your settings, you get the chip only. `images = "off"` does the same.

**0.9.40: screenshots in your notes**

- **⌥V pastes a screenshot.** Copy an image (⌘⇧⌃4 on a Mac), then press ⌥V while writing. The image is saved in the vault, and a line for it appears where you are. With no image on the clipboard, ⌥V still makes the next paste plain text.
- **Drag a file onto the terminal** to attach it: thc asks `attach shot.png? y yes · n paste the path`.
- **An attachment shows as a chip**, `▣ caption · 1280×720 · 240 KB · ⏎ open`. Enter, ⌃O or a click opens it in your viewer.
- **`thc attach <id> <file> --caption "…"`** attaches from the command line. `thc show --json` lists attachments with their full path, so agents can read them.
- `thc doctor` notices a missing attachment, or a file nothing refers to (`--fix --yes` moves it aside, never deletes). Files over 20 MB are refused (`[attachments] max_mb`).
- Inline images in WezTerm come next.

**0.9.39: Mac editing keys**

- **⌘←/→** go to the start or end of the line, **⌘↑/↓** to the start or end of the page or day, and **⌥←/→** move by word. Add **⇧** to any of them to select. **⌘⌫** deletes to the start of the line, and **⌥⌫** deletes a word.
- **In WezTerm, run `thc setup wezterm --yes` once.** It writes thc's own `~/.config/wezterm/thc_keys.lua` and tells you the one line to add to your wezterm.lua. It also turns on WezTerm's kitty keyboard mode, so ⇧Enter makes a line break inside an item. thc never edits your wezterm.lua.
- In kitty, and in WezTerm with the kitty keyboard on, the ⌘ keys work as themselves.

**0.9.38: vaults stay separate, and issues open as pages**

- **Fixed: opening something in another vault could take you back to your home vault.** In a vault you'd switched to, Enter on a page, a note or a task could jump to personal if Today had shown a personal task in the same spot. Now only the vault picker, `--vault`, a project folder, or a row that really belongs to another vault changes the vault.
- **An issue opens as its own page.** Enter on a task with notes under it, or `⌃O` on its line, opens it with the task line as the header (its box, `◆ owner`, priority, when it was opened) and its notes below. Write there to add findings. Esc goes back where you came from.

**0.9.37: what an agent finished waits for you**

- **`is:to-review` finds what an agent marked done that you haven't accepted yet** (in `thc review` or the Log's review lane). Once you accept it, it's simply done. If the agent reopens it and finishes it again, it comes back.
- It's the To review section of an issues board: `thc view set issues --section "To review" 'parent:"¶ Issues" is:to-review' …`.

**0.9.36: a vault can say where captures go**

- **A capture target per vault.** Put `[capture] target = "¶ Issues"` in a vault's `settings.toml`, and what you capture there without naming a place lands on that page. That covers `thc add`, `todo` and `remind`, the capture box and ThoughtBar. Naming a place (`--journal today`, `--under`) still wins.
- **Queries take page titles:** `under:"¶ Issues"` and `parent:Plans`, not only ids.
- **Comparing two versions in a page or day** labels the choices `keep yours` and `keep claude's`.

**0.9.35: Tab never gets stuck, and old windows tell you to reload**

- **Tab goes all the way round.** Arriving on Journal or a page by Tab, a digit or Enter, the document waits: Tab and ⇧Tab keep moving between views. Any other key, or a click in the text, starts writing, and nothing you type is lost.
- **An open thc window notices when you update.** If `thc update` ran elsewhere, the bar says `thc 0.9.35 installed · :update reloads here`. `:update` reloads in place, keeping your view.
- **Two of the same token on one line** (`!high … !low`, `due:fri … due:mon`):
  - On the command line, thc asks you to keep one (or quote them).
  - While you write, the last one counts. The earlier one stays as plain words, and the hint says `2 priorities · using !low`.
- **`thc add --plain`** keeps `#words` as text, with no tags.

**0.9.34: Tab always moves between views**

- **Arriving on a view never takes the cursor.** On Pages and Search the find box shows your last search, with no cursor until you type.
- **On Pages and Search, typing finds:**
  - Every letter goes into the find, and `/` starts an empty one.
  - Move with ↑↓, and open with Enter, → or a double-click.
  - With the box empty, `1`–`7`, `?`, `:`, Space and `q` work as everywhere else.
- **Tab and ⇧Tab always go to the next or previous view,** keeping what you'd typed.
- **Esc** clears the find, then goes back.
- **Page Up and Page Down** move a full screen of the page (less two lines), however much text is showing.
- **The Tasks filter** opens with `f` or `/`, and Esc closes it without wiping your query.
- `6` goes to Search without opening the box; `/` searches.
- The writing cursor is a steady bar (it blinked).

**0.9.33: arrows that go where you look**

- **The arrow keys follow what's on screen.**
  - ↓ from the start of a wrapped line goes to the next line. It no longer sticks on the same line or jumps to the end of the next note.
  - ↑ and ↓ keep the column you started in, through short lines and indented bullets.
- **Across a wrapped line:** End stops after the line's last word, and → goes to the next line in one press.
- **⌃Home and ⌃End** go to the start and end of the page or day.
- **Page Up and Page Down** move a screenful, less two lines, and the view keeps two lines of context around the cursor.
- **Folded notes** are skipped by the arrows.
- **Clicks:**
  - Clicking past the end of a wrapped line puts the cursor at that line's end, not at the start of the next.
  - Clicking a blank gap puts the cursor at the end of the line above.
- **From 0.9.32:**
  - Typing in a page you just made with **+ new page** shows as you type.
  - Each page and day remembers where your cursor was.

**0.9.6 to 0.9.32, in brief**

- **0.9.32:** typing in a new page shows as you type; each page and day remembers your cursor.
- **0.9.31:** a page you click open takes typing at once; a quoted `[[link]]` makes no page; `thc add --plain`.
- **0.9.30:** text in quotes is never read as a token, tag or link; Today and Agenda have sections you can edit.
- **0.9.29:** one background service serves every vault.
- **0.9.28:** Today and Agenda show every vault.
- **0.9.27:** queries across vaults (`vault:acme`, `vault:*`).
- **0.9.26:** a vault picker in the app (⇧V).
- **0.9.25:** a vault can carry its own settings and colour.
- **0.9.24:** a line whose parent was deleted on another device moves here and is flagged, never lost.
- **0.9.23:** vaults: `thc vault` to make, name and switch them.
- **0.9.22:** a rail of pages or days beside a document.
- **0.9.21:** the Pages tab is always the list; Esc goes back where you came from.
- **0.9.20:** very long pages open instantly.
- **0.9.19:** a crash or a closed window never loses what you typed.
- **0.9.18:** a faster editor.
- **0.9.17:** eleven editor bugs found by a fuzz test, fixed.
- **0.9.16:** ⌃O goes back.
- **0.9.15:** a key remap that's refused is explained when you start.
- **0.9.14:** remap keys per context in your settings.
- **0.9.13:** the Space leader and its key hints in lists.
- **0.9.12:** help scrolls; ⌥A selects all.
- **0.9.11:** help, the palette and `thc keys` always match your keys.
- **0.9.10:** the footer shows your actual keys.
- **0.9.9:** one table of keys behind everything.
- **0.9.8:** the history row shows what changed.
- **0.9.7:** the detail pane and list chips are clickable; lists have scrollbars.
- **0.9.6:** ⌃O finds pages and days and creates pages; `:mouse on` and `:mouse off`.

**0.9.5**

- **Panels are clickable.**
  - Help (`F1`) works as a menu: click a key's row to run it.
  - In `:focus`, click an element or a preset.
  - The keys along a panel's bottom edge (`1 keep yours · b both · Esc later`) are buttons.
- **Scrollbar:** a long day or page shows one at the right edge. Click the track to move a page, or drag the thumb.
- **Hover:** in a local terminal, the button under the pointer (a key, a tab, a box, a link, a date) lights up. Over SSH it's off unless you set `hover = "on"`.
- In focus, click a day in the month calendar to go there.
- Clicking a date in the margin opens its editor as you'd type it (`fri`, `oct 9`), not as `2026-10-09`.

**0.9.4: everything clickable**

- **Tabs, footer keys and list rows are clickable.** Click a tab to switch views, a key in the bottom row to run it (`F1 keys`, `⌃T task`…), and a row to select it (double-click opens it). Clicking a task's box in any list marks it done.
- **In a day or page:**
  - Click a day in the strip to go there.
  - With the cursor in a `[[link]]`, the margin shows `↗ open`: click it to open the page.
  - Click a date in the margin to change it.
  - Click a `≠` to compare the two versions.
- **Clicking outside the command palette (or any panel) closes it,** and nothing happens underneath.
- The wheel moves through lists.
- Fixed: the gap after a task's box no longer toggles it. Only the box itself does.

**0.9.3: the mouse in documents**

- **Click** to put the cursor where you click, even on a wrapped line or a wide character. **Drag** to select, **double-click** a word, **triple-click** a whole note, **⇧-click** to extend.
- **Click a task's box** to mark it done, or open again.
- **⌃-click** or **middle-click** a `[[link]]` to open it. A plain click just puts the cursor there.
- **The wheel scrolls without moving the cursor.** Start typing and the view comes back to it.
- Your terminal's own selection is one modifier away (⇧-drag in WezTerm, kitty and Ghostty; ⌥-drag in iTerm2 and Terminal.app). thc says so the first time you drag.
- `mouse = false` in `[tui]` turns capture off entirely. `hover` and `wheel_rows` are there too.
- In the guide's screenshots the clock is pinned, so they stay the same from release to release.

**0.9.2**

- The keys list (`F1`) fits its columns: no label runs into the next one.
- For the guide's screenshots: snapshots show the screen after a line you just left has folded its dates into the margin, and draw the cursor only where there is one.

**0.9.1**

- `[[Helth]]` offers the page you probably meant (`⌃O Health?`) as soon as you close the link, so `⌃O` fixes it before any new page is made.
- A task turned back into text (`⌃T` from done) no longer shows its done time.

**0.9.0: writing v2**

Writing in a day or a page is now plain text: you type, and thc keeps the notes for you.
- **Enter is a line break.** A blank line (Enter twice) starts a new note. Lines starting with `- `, `1. `, `[ ] ` or `#` are items, numbered items, tasks and headings, and Enter on an empty item ends the list.
- **Backspace at the start of a paragraph joins it to the one above.** On a task, Backspace takes the box off, then the bullet.
- **The keys:**
  - `⌃T`: text → `[ ]` → `[x]` → text, on the line or on every line you've selected.
  - `⌃O`: open the link under the cursor, or go to a page.
  - `⌃P` / `⌃N`: the day before or after.
  - `Esc`: save and go to Today.
  - `⌃Q`: quit.
  - `F1`: all the keys.
  - `⌃Z` / `⌃Y`: undo and redo.
  - `⌃C` / `⌃X`: copy and cut.
- There's no separate command mode in documents any more: the single-letter commands live in the lists (Today, Tasks), and `⌃C` no longer quits.
- **Links:** typing `[[Lisbon]]` creates the page when the line saves, and `[[lisbon]]` links an existing `Lisbon`. A near miss (`[[Lisbn]]`) offers the page you probably meant: press `⌃O` to take it.
- A task turned back into text loses its date and repeat along with the box.
- **Fixed:** editing a line you'd just written (leave it, then come back) could mark it changed on two devices (`≠ this device`).
- The days and pages you already have open exactly as before, and opening them never changes them.

**0.8.11**

- **Fixed: capital letters in WezTerm with `enable_kitty_keyboard`** (and in kitty and Ghostty). thc now asks the terminal for the smallest keyboard mode, so every letter, capital and symbol arrives as the text you typed. Caps Lock, other keyboard layouts and accented keys work too.
- **⌃T turns a task on and off** while you type, the same key both ways. **⌃Enter** marks it done or open again.
- **Backspace at the start of a task turns it back into plain text**, for good: before, it came back as a task when saved.
- The keyboard is put back as it was if thc crashes, and while `e` hands a line to your $EDITOR.

**0.8.10**

- **Typing `1. ` starts a numbered list**, as pasting one does: the number moves into the margin, Enter numbers the next item, and Enter on an empty item ends the list.
- The bottom row shows its fullest version whenever it fits (at 100 columns it no longer drops the page name and keys).
- Undo uses far less memory on long pages: each step keeps only the lines it changed, instead of a copy of the whole page (a long session on a big page could use gigabytes).

**0.8.9**

- **Emoji, flags and accents behave like single characters.** The cursor sits right after them, instead of drifting several columns or landing on the next letter. Backspace and Delete remove the whole character (👨‍👩‍👧, 🇯🇵, é), and the arrow keys step over it in one press. Lines containing them wrap where they should.
- Lines below no longer jump while you type a date or tag on a line.
- Headings and numbered items wrap at the same width as the rest of the text, and a word that exactly fills a line stays on it.

**0.8.8**

- **A calmer screen.** thc no longer re-sends the cursor every quarter second while you're idle, which restarted its blink and could make it flicker. Each screen update now reaches the terminal as one piece, so scrolling and switching pages don't tear (WezTerm, kitty, Ghostty, iTerm2, recent tmux).
- **The cursor shows the mode:** a bar while you're typing, your terminal's own cursor otherwise.
- **Keeps up with fast typing:** keys that arrive together are handled before the next redraw, so holding a key no longer lags.
- In a Mac terminal without "Option as Meta", the first ⌥ key that arrives as a character (like `Ω`) shows how to turn it on. Inside tmux, if tmux holds Esc for a long time, thc says how to shorten it.

**0.8.7**

- **Fixed: capital letters in kitty-protocol terminals.** In kitty, Ghostty and WezTerm with `enable_kitty_keyboard`, a shifted letter could arrive as lowercase. Every capital now comes through.
- **Fixed: keys that did more than they should.**
  - While typing, a key combination that isn't a writing key (like `⌥X`) no longer marks the line done or opens a prompt. It does nothing, and says once where commands are (Esc).
  - In lists, `⌃X` and the like no longer act as `x`.
  - A command picked from the palette while typing acts on the line instead of being typed into it.
  - The compare and focus panels ignore key combinations, and `⌃C` closes them.
  - A two-key command cancels cleanly when the next key is an arrow.
- **Fixed:** after ⇧Enter (or `⌃J`) at the end of a line, the cursor now moves to the new line instead of staying on the one above.
- **Fixed:** pasting a single line with a tab in it no longer leaves an invisible character.

**0.8.6**

- **A friendlier first run:** after `thc setup` (and the install script) there's an ending: where your notes live, and `thc j` to start writing. A new, empty vault opens on a short welcome, the first journal day says `just type`, and re-running setup when nothing changed is one line.
- **256-colour terminals** (Terminal.app, tmux, ssh) now get the full palette in 256 colours instead of 16. thc picks it on its own; `THC_THEME=ember-dark-256` or `ember-light-256` chooses it.
- You're `you` everywhere, never `human`. Agents still show as `◆ name`.
- Repeats read the same in lists and documents: `↻ 3d`, `↻ 3mo!`, `↻ monthly`.
- A line changed on two devices names who changed it (`≠ claude`, or the other device), and while you're typing it says `Esc c compare`.
- The command list (`:`): `Update thc` and `What's new` in plain words, and your last three commands first.
- Tasks: the saved filters row ends with `+2 more` rather than cutting one off, a context shows in the bar as it does on Today, and grouping by page no longer repeats the page on every row.
- `thc doctor --fix` reads more plainly (`1 day appears twice (Sun Oct 4)`).

**0.8.5**

- **Ready for more than one Mac:** a journal day, a tag or a page created on two devices before they sync is now one day, one tag or one page afterwards, with everything from both.
- **`thc doctor --fix`** finds days and tags that older versions made twice, plus empty tags left by old headings. It shows what it would merge without changing anything. `thc doctor --fix --yes` makes the change as one step, and `thc undo` puts it all back.
- Fixed: `thc update` sometimes left the background process on the old version, and `thc daemon restart` could leave two running. A restart now leaves exactly one running, on the new version, and says so if it can't.

**0.8.4**

- **Focus is yours to compose.** Choose what focus shows: the keys footer, the date, the day strip, a month calendar, dates and priorities, today's other tasks, links, the view tabs, a word count, a clock, dimming, typewriter scrolling.
- Three presets: `bare` (only your text), `writer` (the new default: the date, the day strip, the keys footer and a word count) and `planner` (adds dates and priorities, a month beside the text from 120 columns, today's tasks, links and a clock, in a slightly narrower column). For focus exactly as in 0.8.2, set `preset = "bare"` in `[tui.focus]`.
- `:focus` opens a small panel: letters show or hide each element as you watch, `1` `2` `3` pick a preset, Enter saves to `~/.config/thought/config.toml`. You can also type `:focus planner`, `:focus +month -footer`, `:focus save` or `:focus off`.
- Settings live in `[tui.focus]` in config.toml, which thc fills in with every option and what it does. A misspelt setting is pointed out with a suggestion.

**0.8.3**

- **A keys footer in the editor:** the bottom row of a journal day or page now shows the keys you need right then, and that saving happens on its own (`autosaved`; `◌ saving…` if a save is slow, `not saved · :retry` if one fails). It differs between writing and commands (Esc), and fits narrow windows.
- **Help knows the editor:** in a document, `?` (after Esc) opens with the writing keys and the document commands, in the same words as the footer. While typing, `⌥?` or F1 opens it. Press `?` again for every key and terminal notes. Help now also opens in focus.
- When a `.thc.toml` in a folder picks the vault, `thc prime`, `thc today` and the TUI now say which file it was (`vault ~/x (from ~/code/x/.thc.toml)`). `thc daemon status` and `thc doctor` give the full path too.
- A `.thc.toml` that points at a folder with no vault in it is now an error that names the file. Before, thc quietly used an empty vault.
- `thc init` won't write a `.thc.toml` in `/`, `/tmp` or your home folder: one there would send every command run anywhere under that folder to its vault.

**0.8.2**

- **Focus:** `thc j` now opens with nothing on screen but what you're writing: no tabs, dates, calendar strip, sidebars, footers or status bar (bullets and checkboxes stay). Press `⌥Z` while typing, or `F` after Esc, to switch it on and off. `thc j --no-focus` opens the full screen.
- **Settings:** how `thc j` and `thc p` open, and the focus options, live in the `[tui]` section of `~/.config/thought/config.toml`, which thc now fills in with every option and what it does.
- Fixed: in a journal day, `x` (and the other single keys) kept acting on the first line you'd selected instead of the line under the cursor.
- Fixed: tags added with `thc tag` or `thc todo -t` disappeared the next time the line's text changed.

**0.8.1**

- Code blocks keep their lines: long lines no longer wrap, they end in `→` and scroll sideways while you're in them.
- Typing stays quick even when thc's background process isn't running: a line saves just after the screen updates.
- `thc update` also refreshes the thc files it installed for Claude Code and Codex.

**0.8.0**

Write in the terminal. A journal day or a page in `thc tui` is now one document you type into.

- **`thc j`** opens today's journal ready to type, with the caret on a fresh line at the end (`thc j yesterday`, `thc j fri`, `thc j 2026-10-02`). **`thc p Q4 Planning`** opens a page.
- **Writing:** Enter keeps the line's form (a bullet after a bullet, the next number after `1.`); an Enter on an empty bullet steps out. `- ` and `[ ] ` make bullets and tasks, Tab / ⇧Tab nest, ⌃T makes a task, completes it, reopens it, ⌥↑ / ⌥↓ move a line with its children, ⌃J breaks a line without a new note. Readline keys (⌃A ⌃E ⌃W ⌃K ⌃U), word moves (⌥← ⌥→), ⇧ to select across lines, ⌃Z / ⌃R undo and redo.
- **Tokens as you type:** `due:fri`, `!high` and `#tags` are underlined, the right margin shows what they'll set, and when you leave the line the dates and priority move into the margin.
- **`[[`** offers pages and days as you type, and only makes a new page when you pick it.
- **Paste** Markdown notes and they become the outline (headings, lists, tasks, quotes, code), as one undo step. ⌃C copies as Markdown; ⌥V makes the next paste plain.
- **Headings, numbered lists, quotes, code and rules** show as you'd expect, their markers dimmed in the margin.
- **Esc** switches to commands: `x` done, `d` due, `space` folds, `V` selects lines, `F` Focus (just the text), and the usual single keys. `i` writes again.
- **Under the day**, *also today* lists what's due elsewhere; under a page, *linked from* lists what mentions it.
- **If another device or an agent changes the line you're on**, it's marked and applied when you leave it; if you both changed it, both versions are kept and `c` compares them, yours first.
- Every line saves itself when you leave it, after a pause, and when you switch away or quit; with the daemon running, saving never slows typing.
- `:update` in the TUI updates thc and puts you back where you were.

**0.7.3**

- Fixed: running `thc setup --migrate-from-app` a second time could leave thc's background process stopped. `thc daemon start` and `thc update` now restart it through its login item, so it stays managed by macOS.

**0.7.1**

- After moving from Thought Central.app, `thc setup --status` shows thc's own background process and the standalone `thc`, not the app's old entries. Running `thc setup --migrate-from-app` again cleans them up.
- Faster with long pages: a page with thousands of lines opens in milliseconds instead of seconds.

**0.7.0**

thc now installs and updates on its own, without the Mac app.

- **Install:** `curl -fsSL https://github.com/brancusi/thought-central-releases/releases/latest/download/install.sh | sh` puts `thc` in `~/.local/bin` and sets up the vault, the background daemon and agent skills. Every download is checked (checksum, Apple signature, update signature) before anything is installed.
- **Update from the terminal:** `thc update` fetches the latest release, verifies it, swaps it in place and restarts the daemon on it. `thc update --rollback` puts the previous version back. `thc today` mentions when a new version is out (the daemon checks every 6 hours; `update = "off"` in ~/.config/thought/config.toml turns that off, `"auto"` installs them).
- **Moving from Thought Central.app:** the installer notices `thc` is a link into the app and takes over: the standalone `thc` replaces the link, the app's login item is switched off, and thc runs its own daemon. You can then delete the app.
- Fixed: `## Heading` lines no longer get a `#` tag, and nothing inside `backticks` or code fences is read as a date, priority or tag.

**0.6.1**

- While you type `due:fri` or `!high`, the right margin shows what they'll set (`due fri · !high ✓`), or what it can't read.
- Under today's journal, **Also today** lists what's due, overdue or done today elsewhere; tick a box to complete it where it lives.
- Under a page, **Linked from** lists the lines that mention it.

**0.6.0**

The window gets Today, Inbox and Tasks.

- **Today** (⌘1) shows what's overdue, today, in progress, coming up and done today. **Inbox** (⌘2) lists what's waiting to be sorted, oldest first. **Tasks** (⌘3) runs any query (`status:open #work sort:due`).
- Keys as in the terminal: `j` / `k` to move, `x` done, `X` reopen, `t` task on or off, `D` delete, `y` copy the id, `Enter` to open the line where it lives, `⌘Z` to undo.

**0.5.1**

- Window polish: even spacing around paragraphs, completed tasks show when they were done, every day with entries gets a dot in the day strip, and the pages list shows how long each page is and when you last edited it.

**0.5.0**

Thought Central now has a window for writing. Click **⌘O OPEN** in the panel (or the version menu › Open Thought Central).

- **Journal:** each day is a document. Just type. Paragraphs, `- ` bullets and `[ ] ` tasks; Tab nests; ⌘↩ makes a line a task or completes it. `due:fri`, `!high` and the like move into the right margin when you leave the line; `#tags` stay where you wrote them. ⌘[ and ⌘] move between days, ⌘J goes to today.
- **Pages:** open any page from the rail or with ⌘O, and write in it the same way.
- Every line saves itself as you go (when you leave it, after a short pause, and when you switch away). If an agent or another Mac changes a line while you're writing it, both versions are kept.
- Today, Inbox, Tasks, Search and Log come to the window in the next updates; the panel and the terminal have them now.
- The panel's capture error is shorter: `"fryday" isn't a date · ⏎ saves it as text`.

**0.4.3**

- Fixed: moving or placing a note between two notes added on different devices at the same time could stop thc's background process (and `thc mv --after`) with a crash. Nothing was written wrongly; it now places the note correctly.

**0.4.2**

- Quick capture shows its preview line again while you type, and a date it can't read is underlined in red.
- Polish from the design review: the next-7-days section reads NEXT 7D, overdue rows keep a plain status cell, shortcuts show as ⌃⌥SPACE, the digest time is 24-hour, and VoiceOver no longer reads the connection error twice.

**0.4.0**

A new look: Thought Central now reads like the terminal it comes from.

- Square, dense and monospaced (IBM Plex Mono, bundled), with one accent colour. Night is the default; choose Day or follow the system in Settings › General.
- The bar at the top shows the view (click it for your saved views), the time and whether thc is live. The footer has ⌘T for the terminal, ⌘, for Settings and the version menu.
- Rows are 22 pt, and each row's status is the click target: `[ ]` to finish, `[≠]` to compare a conflict. Hovering shows SNOOZE, ACK and ↗.
- When thc can't be reached, the error shows in a red frame with thc's own words and RETRY.
- Settings and the setup window use the same style.

**0.3.1**

- Fixed: if you'd used Thought Central before 0.3.0, setup didn't run, so the `thc` command, the login item and agent skills weren't added. It now runs whenever nothing has been set up yet, and keeps your existing vault.
- After an update, setup checks itself quietly and only opens its window if something needed doing.

**0.3.0**

Install once and everything is ready: no Start button, no setup steps.

- First launch sets everything up in one window that ticks itself off: your vault (`~/thought`, unless you have one), the `thc` command in your terminal, thc running in the background and at login, and the `thc` skill for Claude Code and Codex if you use them. The only question is the one macOS asks about notifications.
- The `thc` command goes in `~/.local/bin`, with one marked line in your shell profile only if a new terminal wouldn't find it. A `thc` you already have (Homebrew, cargo) is never replaced. Settings › Setup has **Install for all users…** if you'd rather have it in `/usr/local/bin`.
- `install.sh` ends in the same state, and prints the same checklist.
- Running `thc` for the first time on a new Mac sets it up once, asking only whether to teach your agents. Scripts, agents and CI never trigger this.
- `thc setup --status` shows what's set up. `thc setup --undo`, or **Remove everything…** in Settings › Setup, removes exactly what setup added. Your notes are never touched.
- After an update, agent skills are refreshed and the footer says so.

**0.2.5**

- The panel's height follows what's in it: at least 240 pt, at most 640 pt (less on a small screen), and only the list scrolls. A thin line shows where more rows are hidden, and resizing is animated unless Reduce Motion is on.

**0.2.4**

- Fixed: the panel stayed short and hid today's list behind its footer. It now grows to fit what's due, up to 640 pt (or 80% of a small screen), and scrolls beyond that.

**0.2.2**

Thought Central now sets itself up. There's no Start button any more.

- First launch makes your vault in `~/thought` (never a sync folder) unless you already have one. You can move it anytime in Settings › Vault, which says where the vault came from.
- thc runs in the background by itself and starts at login (System Settings › Login Items lists it as Thought Central). If it stops, Thought Central starts it again.
- When something goes wrong you see thc's own error, with **Retry** and **Copy details**, in the panel, in Settings › Background and in onboarding. No more buttons that silently do nothing.
- Onboarding is three short steps: welcome, your vault, then reminders and the shortcut.
- `thc doctor` and `thc daemon status` now say which vault they're using and why (`from .thc.toml in …` or `from ~/.config/thought/config.toml`). With no vault at all, thc suggests `thc init ~/thought --global`.
- `thc daemon start` now shows the daemon's own error when it can't start.

**0.2.1**

ThoughtBar is now **Thought Central**, with its own app icon. Please download it once more; from here on it updates itself.

- Fixed: 0.1.0 and 0.2.0 quit at launch without showing the menu bar icon. Every release is now launched on the build machine, with the build files hidden, before it's published.
- Lists stay lists: a note you wrote as a bullet keeps its `- ` (and its `·` in the terminal); only notes you write as paragraphs read as prose.
- New lines take the form of the line you were on, and typing `- ` at the start of a new paragraph line makes it a bullet.

**0.2.0**

Pages and journal days now read and edit like an outliner.

- Press `i` to edit a line where it sits, or `o` / `O` to start a new one below or above. Lines save when you leave them; `Enter` keeps writing, and `Tab` nests.
- Tokens work while you type: `due:fri`, `!high` and `#tag` are applied when the line saves, with a preview at the right.
- Paragraphs read as prose. Notes on a page wrap instead of being cut off, and in `$EDITOR` a paragraph wrapped over several lines stays one note.
- IDs are hidden on pages; press `.` to show them.
- A stray key in the page finder no longer creates a page: choose `+ new page` with `↓`, then `Enter`.
- Fixed: the Markdown export (`thc export`) stopped writing the agenda in 0.1.0's speed work.

**0.1.0**

The first release of ThoughtBar, the menu bar app for thought-central, with its own `thc`.

- Today, upcoming and the inbox in a 360 pt panel, with quick capture (`due:fri #tag !high`) and the same parser as the CLI.
- Alerts as notifications: done, snooze or acknowledge from the banner.
- Agent changes are labelled, counted in the footer and reviewable in the terminal (`thc review`).
- Sync conflicts, contexts and saved views (presets) shown as they are in the TUI.
- **Install command line tool** (Settings → Command line) links `thc` to the app's copy, so updates keep it current.
- Updates itself: signed, notarized releases, checked every few minutes.
