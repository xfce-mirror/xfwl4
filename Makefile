.PHONY: all build build-dev install install-dev uninstall _install check install-git-hooks

TARGET = xfwl4

PREFIX = /usr/local
BINDIR = $(PREFIX)/bin

INSTALL = install

ifeq ($(XFWL4_FEATURES),)
	CARGO_FEATURES_ARG =
else
	CARGO_FEATURES_ARG = -F $(XFWL4_FEATURES)
endif

ifneq ($(filter %-dev,$(MAKECMDGOALS)),)
	BUILDTYPE = debug
else
	BUILDTYPE = release
endif

all: build

build:
	cargo build --release $(CARGO_FEATURES_ARG)

build-dev:
	cargo build $(CARGO_FEATURES_ARG)

install: build _install

install-dev: build-dev _install

_install:
	$(INSTALL) -d $(DESTDIR)$(BINDIR)
	$(INSTALL) -m 0755 target/$(BUILDTYPE)/$(TARGET) $(DESTDIR)$(BINDIR)

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(TARGET)
	rmdir -p $(DESTDIR)$(BINDIR) 2>/dev/null || true

check: install-git-hooks

install-git-hooks: $(addprefix .git/hooks/,$(notdir $(shell find .githooks/ -type f)))

.git/hooks/%: .githooks/%
	$(INSTALL) -d .git/hooks
	$(INSTALL) -m 0755 $< $@
