# Rollback Protocol

Rollback accepts `chk_<id>`, recoverable staged `pck_<id>`, and signed receipt references `rcp_<id>`. After staging disposal, the immutable pack remains authoritative but is not itself a mutable recovery snapshot; Draft points to its signed submit receipt, which binds the dedicated rollback target.
