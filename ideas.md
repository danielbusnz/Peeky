# Ideas

Feature ideas worth building. The bar: would a recruiter stop scrolling, and would I leave it running all day.

## 1. Demonstration mode (ghost cursor)

"Show me how to set up a reverse proxy in this UI" and a ghost cursor walks the real steps slowly in the actual app while narrating each one. Teaching by demonstration instead of automation.

Why it's unique:
- Uses the moat directly: eyes (sees the real UI) + movement (gestures through it), composed into something neither does alone.
- Zero-risk version of computer use. It never clicks anything destructive, it only points and narrates. The safety dial is at zero while still demoing the full perception-to-motion pipeline.
- Different product category than Clicky's agent automation. They do the task for you; this teaches you in your own apps. Stickier, and nobody on Linux has it.
- Demo video writes itself: split second of voice, then the cursor glides through a real app step by step with narration.

Rough shape: query -> vision model plans steps against live screenshots -> cursor moves to each target and dwells/circles while TTS narrates -> waits for user "next" or auto-advances. No click events dispatched, motion and speech only.
