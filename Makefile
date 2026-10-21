DESTDIR ?=
DEVICE ?=

# Services every device runs. Each device enables them in its rootfs/.
EVERY_DEVICE = services/memory/zram-swap

.PHONY: install-device
install-device:
	@test -n "$(DEVICE)" || { echo "usage: make install-device DEVICE=<codename> [DESTDIR=path]" >&2; exit 1; }
	@test -d devices/$(DEVICE) || { echo "unknown device: $(DEVICE)" >&2; exit 1; }
	@for service in $(EVERY_DEVICE); do $(MAKE) -C $$service install DESTDIR=$(abspath $(DESTDIR)) || exit 1; done
	@if [ -d devices/$(DEVICE)/rootfs ]; then \
		cp -r devices/$(DEVICE)/rootfs/. $(DESTDIR)/; \
	else \
		echo "$(DEVICE) has no files to install"; \
	fi
