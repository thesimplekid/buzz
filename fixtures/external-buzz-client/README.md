# External `buzz-client` compile fixture

This deliberately independent Cargo project verifies the supported
exact-revision Git dependency boundary. Its own empty `[workspace]` table keeps
it outside the Buzz workspace even though the fixture is stored in this
repository.

The pinned revision contains `buzz-client` and its `buzz-core`, `buzz-sdk`, and
`buzz-ws-client` workspace dependencies. When changing the shared crate graph,
update the revision to a committed Buzz change and run:

```bash
cargo check --manifest-path fixtures/external-buzz-client/Cargo.toml
```

For an unpushed local change, Git's `url.<local-repository>.insteadOf` setting
can temporarily redirect the GitHub URL to the local repository. Do not commit
that machine-specific redirect.
