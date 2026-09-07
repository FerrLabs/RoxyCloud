# A sync-only mode, where the server stores nothing

This answers the RFC in #34, which asked whether "a device is a peer, and the server is a peer that
happens to be always on" survives contact with the code, and said it was blocked on #21 landing.

#21 has landed. The framing survives, with one part of it that does not, and the honest scope is
smaller than the RFC guessed.

## The framing survives, because #21 already built the seam

The RFC worried this might be a second product. It is not, and the evidence is in
`crates/client/src/sync/transport.rs`:

```rust
pub trait Transport {
    type Error: std::error::Error + Send + Sync + 'static;
    fn snapshot(&self) -> impl Future<Output = Result<Snapshot, Self::Error>> + Send;
    fn download_to(&self, path: &RelPath, destination: &Path) -> ...;
    fn upload_from(&self, path: &RelPath, source: &Path) -> ...;
    fn remove(&self, path: &RelPath) -> ...;
}
```

The reconciler does not speak REST. `reconcile(local, remote, base, now)` in `sync/plan.rs` takes
three snapshots and answers a plan, and the engine drives that plan through a `Transport`. A peer is
a second implementation of four methods. Nothing in the reconciler names a server, a session token,
or an HTTP status.

That is the answer to the RFC's blocking question: **a variation, not a rewrite.**

## What does not survive

The RFC proposed the mode as "one policy on a peer", with the server holding no blob in `sync` mode.
That reads as a smaller change than it is, because three-way reconciliation needs a base, and the
base is per-pair.

`sync/state.rs` keeps the last agreed state in `.roxycloud-sync.json` beside the folder, one file,
because there is one remote. With N peers there are N bases, and a device that syncs with two others
needs to remember what it last agreed with each. That is a change to the state file's shape rather
than to the reconciler, but it is a change, and pretending the mode is a flag hides it.

So the table in the RFC is right about what each mode offers and wrong about the cost. `store` and
`sync` share the reconciler, the snapshot type, the conflict rule and the content addressing. They
do not share the state file.

## The three questions the RFC left open

**Device identity.** Still the part most likely to be got wrong, and nothing since #21 has changed
that. Session tokens are wrong: there is no server to issue them. This wants a per-device keypair
and an id both ends pin, and it is a third authentication path next to passwords and app passwords.
It is also the one piece with no existing seam in this repository, which makes it the right thing to
prototype first and the wrong thing to design on paper.

**Discovery and reachability.** Unchanged by #21. The first cut should require reachability and say
so, because the moment something relays, the relay sees the traffic, and end-to-end encryption stops
being a v2 concern and lands on the critical path.

**What is absent in sync mode.** This is now sharper than when the RFC was written, because more of
the product exists to be absent. Share links need bytes somewhere. WebDAV needs a host that holds
content. Quotas are a statement about server storage. Trash and restore need a copy that outlives a
local delete. Thumbnails are generated from bytes the server has. Resumable uploads are sessions
against a server's staging area. That is six features, not three, and a mode that quietly degrades
them is worse than one that says which half of the product it is.

## Recommendation

Prototype device identity against the existing `Transport` trait, with everything else stubbed: two
machines, ids pasted between them by hand, one folder, direct connection, no discovery, no relay.
That is enough to find out whether the keypair path is as awkward as it looks, and small enough to
throw away.

Do not start with the mode flag, the deployment shape, or the state file. Those follow from whether
device identity is bearable, and none of them is the risky part.

## Not decided here

Whether to do it at all. The argument for is that two or three machines belonging to one person
wanting the same folder is a real case the current shape serves badly. The argument against is that
six features do not exist in that mode, and a file server that is missing half of itself in one
configuration is a support burden as much as a feature.
