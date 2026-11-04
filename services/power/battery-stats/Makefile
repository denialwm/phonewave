PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# The Rust target of a cross build, such as aarch64-unknown-linux-gnu.
TARGET ?=
BIN = target/$(if $(TARGET),$(TARGET)/)release

.PHONY: all check install
all:
	$(CARGO) build --release --locked $(if $(TARGET),--target $(TARGET))
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN)/battery-statsd $(DESTDIR)$(PREFIX)/bin/battery-statsd
	install -Dm755 $(BIN)/battery-stats $(DESTDIR)$(PREFIX)/bin/battery-stats
	install -Dm644 packaging/battery-statsd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/battery-statsd.service
	sed -i 's|/usr/bin/battery-statsd|$(PREFIX)/bin/battery-statsd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/battery-statsd.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/battery-stats/LICENSE
