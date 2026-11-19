# tessera-network-system

The system side of ADR-0242: Kestrel, NetworkManager, and iwd backends, explicit
selection, and the `tessera-networkd` service that owns the system D-Bus
connection on behalf of the compositor.

The compositor does not link this package. It speaks the protocol in
`tessera-network` over `$XDG_RUNTIME_DIR/tessera-networkd.sock`; the service
chooses a backend from `[network] backend` and reports honest availability.
