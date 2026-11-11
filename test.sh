cd /data/projects/tessera-dev || exit 1
RUST_LOG='tessera::runtime::damage=trace,tessera::runtime::presentation=trace,tessera::runtime::event_loop=trace,tessera_platform::drm::device=debug,tessera::runtime::input=trace,tessera_composite=trace,tessera_wayland::protocol::compositor=trace' \
  ./target/release-fast/tessera 2>&1 | tee /tmp/tessera-trace3.log
