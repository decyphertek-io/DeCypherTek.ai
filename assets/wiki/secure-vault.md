# The Secure Vault

Everything the agent knows — config, API keys, RAG memory, wiki, chat
logs — lives under `~/.decyphertek.ai/` as ONE encrypted file, `vault.dct`.

## Format

    magic("DCTVAULT") | version(1) | salt(16) | nonce(12) | AES-256-GCM ciphertext

The payload is a gzip'd tar of the decrypted session directory. The key is
never stored: it is derived on demand with

    Argon2id(password, salt, m=32MiB, t=2, p=1) -> 32-byte key

AES-GCM is authenticated — a wrong password fails the tag check with no
way to fall through. There is no backdoor and no recovery: lose the
password, lose the agent (by design).

## Lifecycle

1. Launch → password prompt (3 tries).
2. Unseal → plaintext extracted into `staging/` (0700) → shell runs.
3. Exit (or EOF/Ctrl-D) → everything gzips/tars back, encrypts with a
   fresh nonce, atomically replaces `vault.dct`, staging is wiped.

## Crash behavior

If the process dies mid-session, `staging/` survives unsealed. The next
launch checks the password against the sealed vault, then re-seals from
that staging. Worst case: you lose nothing.

## What this protects against

- Stolen phone / copied home folder: the vault is noise without the
  password (Argon2id at 32 MiB makes offline guessing expensive).
- The API keys inside the vault are never written anywhere in plaintext
  outside staging — and `@status` never prints your key.

Use `@password` to re-key the vault any time.
