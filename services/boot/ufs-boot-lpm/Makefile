PREFIX ?= /usr
DESTDIR ?=

.PHONY: check install
check:
	shellcheck ufs-boot-lpm tests/run.sh
	sh tests/run.sh
install:
	install -Dm755 ufs-boot-lpm $(DESTDIR)$(PREFIX)/bin/ufs-boot-lpm
	for unit in ufs-boot-lpm ufs-boot-lpm-release; do \
		install -Dm644 packaging/$$unit.service $(DESTDIR)$(PREFIX)/lib/systemd/system/$$unit.service; \
		sed -i 's|/usr/bin/ufs-boot-lpm|$(PREFIX)/bin/ufs-boot-lpm|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/$$unit.service; \
	done
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/ufs-boot-lpm/LICENSE
