.PHONY: all check install-git-hooks

all:

check: install-git-hooks
	./.git/hooks/pre-commit

install-git-hooks: $(addprefix .git/hooks/,$(notdir $(shell find .githooks/ -type f)))

.git/hooks/%: .githooks/%
	mkdir -p .git/hooks
	ln -sf ../../$< $@
