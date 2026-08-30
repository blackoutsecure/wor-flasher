## What does this change?

<!-- A short description, and the issue it fixes if there is one. -->

## How was it tested?

<!-- Which Raspberry Pi model, which Windows build, and what you observed. -->

- [ ] `bash -n install-wor.sh && bash -n install-wor-gui.sh` passes
- [ ] Tested with `DRY_RUN=1`, or flashed a real drive
- [ ] Raspberry Pi model tested:
- [ ] Windows build tested:

## Checklist

- [ ] The GUI and CLI still agree, if shared logic changed
- [ ] README updated, if behaviour or variables changed
- [ ] No new ShellCheck errors (`shellcheck --severity=error install-wor.sh install-wor-gui.sh terminal-run`)
