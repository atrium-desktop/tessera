# tessera-i18n

`tessera-i18n` owns locale negotiation and the strongly typed translated
message catalog for tessera chrome.

## Responsibilities

- POSIX message-locale precedence negotiation (`LC_ALL`, `LC_MESSAGES`, `LANG`)
  with BCP-47 subtag handling.
- Embedded translation catalogs (`locales/*.toml`); a missing key is a
  startup/test failure instead of a silently leaked identifier.
- The typed `Localizer` surface consumed by every chrome component.

## Non-goals

- No filesystem I/O at render time: catalogs are embedded in the binary.
- No fluent-style runtime interpolation plumbing beyond what the typed
  catalog needs.
