# You and an agent, one document

The agent is a separate client on this editor's socket, speaking the same JSON-lines protocol any script can. It has its own view and caret: the pane below. Every keystroke it sends is a guarded write, so if you typed between its read and its write, the editor refuses it and the agent reads again.

## Yours

Type here while the agent works: a few lines, a list, anything. Then press ⌃Z: undo only takes back your edits, never the agent's.

- 

## The agent's

