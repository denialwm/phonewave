PREFIX ?= /usr
DESTDIR ?=

.PHONY: check install
check:
	shellcheck zram-swap tests/run.sh
	sh tests/run.sh
install:
	install -Dm755 zram-swap $(DESTDIR)$(PREFIX)/bin/zram-swap
	install -Dm644 packaging/zram-swap.service $(DESTDIR)$(PREFIX)/lib/systemd/system/zram-swap.service
	sed -i 's|/usr/bin/zram-swap|$(PREFIX)/bin/zram-swap|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/zram-swap.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/zram-swap/LICENSE
