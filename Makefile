DESTDIR ?=
DEVICE ?=

.PHONY: install-device
install-device:
	@test -n "$(DEVICE)" || { echo "usage: make install-device DEVICE=<codename> [DESTDIR=path]" >&2; exit 1; }
	@test -d devices/$(DEVICE) || { echo "unknown device: $(DEVICE)" >&2; exit 1; }
	@if [ -d devices/$(DEVICE)/rootfs ]; then \
		cp -r devices/$(DEVICE)/rootfs/. $(DESTDIR)/; \
	else \
		echo "$(DEVICE) has no files to install"; \
	fi
