.PHONY: all check install-git-hooks

INSTALL = install

all:

check: install-git-hooks

install-git-hooks: $(addprefix .git/hooks/,$(notdir $(shell find .githooks/ -type f)))

.git/hooks/%: .githooks/%
	$(INSTALL) -d .git/hooks
	$(INSTALL) -m 0755 $< $@
