# Rollback Protocol

Rollback accepts `chk_<id>`, recoverable `pck_<id>`, and stable receipt
references `rcp_<id>`. Disposed packs are not active rollback layers; rollback to
a disposed pack fails clearly and should point to the related receipt when known.
