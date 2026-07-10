# Compatibility Protocol

v0.3.3 migrates v0.3.2 state non-destructively. Migration preserves pending
packs, receipts, events, checkpoints, and config. It must not silently dispose
packs, close the workspace, or advance `stable_head` beyond the verified current
state.
