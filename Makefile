SHELL := /bin/bash
BIN   := tickerstrip
PREFIX?= $(HOME)/.local
PLIST := $(HOME)/Library/LaunchAgents/com.tickerstrip.plist

.PHONY: build release test run install uninstall autostart unautostart snapshot clean fmt

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

run: build
	./target/debug/$(BIN)

# 前台跑 + 每秒打印几何信息
debug: build
	MBQUOTE_DEBUG=1 ./target/debug/$(BIN)

# 把面板渲染成 PNG，用于核对排版（不依赖录屏权限）
snapshot: build
	rm -f /tmp/tickerstrip-panel.png
	MBQUOTE_DEBUG_SNAPSHOT=/tmp/tickerstrip-panel.png ./target/debug/$(BIN) &
	sleep 4; pkill -f 'target/debug/$(BIN)' || true
	@echo "面板快照：/tmp/tickerstrip-panel.png"
	@open /tmp/tickerstrip-panel.png

install: release
	mkdir -p $(PREFIX)/bin
	install -m 755 target/release/$(BIN) $(PREFIX)/bin/$(BIN)
	@echo "已安装到 $(PREFIX)/bin/$(BIN)"
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; *) echo "提示：把 $(PREFIX)/bin 加进 PATH";; esac

uninstall:
	-$(MAKE) unautostart
	rm -f $(PREFIX)/bin/$(BIN)

autostart: install
	@mkdir -p $(dir $(PLIST))
	@printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '  <key>Label</key><string>com.tickerstrip</string>' \
	  '  <key>ProgramArguments</key>' \
	  '  <array><string>$(PREFIX)/bin/$(BIN)</string></array>' \
	  '  <key>RunAtLoad</key><true/>' \
	  '  <key>KeepAlive</key><true/>' \
	  '  <key>StandardErrorPath</key><string>/tmp/tickerstrip.err.log</string>' \
	  '</dict></plist>' > $(PLIST)
	-launchctl unload $(PLIST) 2>/dev/null
	launchctl load $(PLIST)
	@echo "已设为登录自启（$(PLIST)）"

unautostart:
	-launchctl unload $(PLIST) 2>/dev/null
	rm -f $(PLIST)
	@echo "已取消登录自启"

fmt:
	cargo fmt

clean:
	cargo clean
