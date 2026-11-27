PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# The Rust target of a cross build, such as aarch64-unknown-linux-gnu.
TARGET ?=

BIN = target/$(if $(TARGET),$(TARGET)/)release/video-wallpaper

.PHONY: all check install
all:
	$(CARGO) build --release --locked $(if $(TARGET),--target $(TARGET))
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/video-wallpaper
	install -Dm644 systemd/video-wallpaper.service \
		$(DESTDIR)$(PREFIX)/lib/systemd/user/video-wallpaper.service
	install -d $(DESTDIR)$(PREFIX)/lib/systemd/user/graphical-session.target.wants
	ln -sf ../video-wallpaper.service \
		$(DESTDIR)$(PREFIX)/lib/systemd/user/graphical-session.target.wants/video-wallpaper.service
