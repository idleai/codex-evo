# Portable history runtime

This package owns the existing history peer coordinator, discovery, invitations,
sharing scope, progress and native worker bridge. It was extracted from the
EditChain extension without changing the invitation, worker or saved-state
formats.

Hosts inject the relay transport, credentials, persistence and app-core peer-state
factory. The VS Code compatibility adapter lives in
`vscode-extension/extensions/vscode-editchain/src/multiplayer`; it supplies Dev
Tunnels, SecretStorage and the packaged peer-state WASM module. The package has no
VS Code API dependency.

```sh
npm ci
npm test
```

The VS Code renderer workflow also runs the complete hosted sharing, reconnect,
cleanup and native replication integration suite against this package. That host
uses a local npm file dependency and includes its compiled `dist` in the VSIX.

The f18 standalone Evo coordination service and its configuration/provider
connections remain separate roadmap work. This move does not start a service or
connect any account by itself.
