PREFIX ?= /usr
DESTDIR ?=

.PHONY: check install
check:
	shellcheck rtc-offset tests/run.sh
	sh tests/run.sh
install:
	install -Dm755 rtc-offset $(DESTDIR)$(PREFIX)/bin/rtc-offset
	install -Dm644 packaging/rtc-offset.service $(DESTDIR)$(PREFIX)/lib/systemd/system/rtc-offset.service
	install -Dm644 packaging/rtc-offset-sync.service $(DESTDIR)$(PREFIX)/lib/systemd/system/rtc-offset-sync.service
	install -Dm644 packaging/rtc-offset-sync.path $(DESTDIR)$(PREFIX)/lib/systemd/system/rtc-offset-sync.path
	sed -i 's|/usr/bin/rtc-offset|$(PREFIX)/bin/rtc-offset|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/rtc-offset.service $(DESTDIR)$(PREFIX)/lib/systemd/system/rtc-offset-sync.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/rtc-offset/LICENSE
