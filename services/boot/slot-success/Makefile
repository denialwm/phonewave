PREFIX ?= /usr
DESTDIR ?=
CC ?= cc
# qbootctl writes the slot attributes: linux-msm's port of the Qualcomm boot
# control HAL that Android runs, built unmodified from this commit.
QBOOTCTL_URL ?= https://github.com/linux-msm/qbootctl.git
QBOOTCTL_COMMIT ?= 39a6e6daaf029fb0a083777679a15ea2c18f72de
QBOOTCTL_SRC ?= build/qbootctl-src
QBOOTCTL_SOURCES = qbootctl.c bootctrl_impl.c gpt-utils.c ufs-bsg.c crc32.c

.PHONY: all check install
all: build/qbootctl
$(QBOOTCTL_SRC)/qbootctl.c:
	git clone --quiet $(QBOOTCTL_URL) $(QBOOTCTL_SRC)
	git -C $(QBOOTCTL_SRC) checkout --quiet $(QBOOTCTL_COMMIT)
build/qbootctl: $(QBOOTCTL_SRC)/qbootctl.c
	test "$$(git -C $(QBOOTCTL_SRC) rev-parse HEAD)" = $(QBOOTCTL_COMMIT)
	$(CC) -std=gnu11 -O2 -static -I$(QBOOTCTL_SRC) -o $@ $(addprefix $(QBOOTCTL_SRC)/,$(QBOOTCTL_SOURCES))
check:
	shellcheck slot-success tests/run.sh
	sh tests/run.sh
install:
	install -Dm755 slot-success $(DESTDIR)$(PREFIX)/bin/slot-success
	install -Dm755 build/qbootctl $(DESTDIR)$(PREFIX)/lib/slot-success/qbootctl
	sed -i 's|/usr/lib/slot-success/qbootctl|$(PREFIX)/lib/slot-success/qbootctl|' $(DESTDIR)$(PREFIX)/bin/slot-success
	install -Dm644 packaging/slot-success.service $(DESTDIR)$(PREFIX)/lib/systemd/system/slot-success.service
	sed -i 's|/usr/bin/slot-success|$(PREFIX)/bin/slot-success|' $(DESTDIR)$(PREFIX)/lib/systemd/system/slot-success.service
	# Installed means on: the unit has no [Install] section to disable.
	install -d $(DESTDIR)$(PREFIX)/lib/systemd/system/multi-user.target.wants
	ln -sf ../slot-success.service $(DESTDIR)$(PREFIX)/lib/systemd/system/multi-user.target.wants/slot-success.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/slot-success/LICENSE
	install -Dm644 $(QBOOTCTL_SRC)/LICENSE $(DESTDIR)$(PREFIX)/share/licenses/slot-success/qbootctl-LICENSE
