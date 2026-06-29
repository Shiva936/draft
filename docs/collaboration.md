# Collaboration

Draft v0.2.0 is **local-first**. It establishes the foundation for collaboration without shipping networked features:

- A portable, provider-neutral record of intent (changes, reviews, risk, verification, receipts) lives in `.draft/`.
- Actors (human/agent/service) are attributed on every operation and receipt, so human + AI-agent workflows are first-class.
- The `services/sync` crate is a reserved placeholder; remote/cloud sync, hosted review, and real-time CRDT collaboration are **out of scope** for v0.2.0 (see roadmap and known limitations).
