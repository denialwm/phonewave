PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# The Rust target of a cross build, such as aarch64-unknown-linux-gnu.
TARGET ?=

BIN = target/$(if $(TARGET),$(TARGET)/)release/screen-record

.PHONY: all check install
all:
	$(CARGO) build --release --locked $(if $(TARGET),--target $(TARGET))
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
install:
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/screen-record
