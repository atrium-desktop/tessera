# tessera-network

Backend-neutral network vocabulary and the control protocol shared by the
compositor and `tessera-networkd` (ADR-0242).

This crate is pure: it has no D-Bus, no NetworkManager, and no `zbus`
dependency. A network service is whatever answers the protocol; today that is
NetworkManager or iwd behind `tessera-network-system`, but the compositor
cannot tell which.
