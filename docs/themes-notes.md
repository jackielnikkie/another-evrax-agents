# Referensi tema (hex lengkap)

Dipindah dari AGENTS.md agar file itu ramping. Sumber aset: palet internal v2 (`crates/evrax-tui/src/theme.rs`).

## Mapping 5 tema baru
Formula: accent=signature chrome, interactive=sekunder, selection/action_dim=surface tint, raised=backgroundPanel, md/syn/diff persis aset.

### catppuccin (verified penuh)
base `#1e1e2e`, raised `#181825`, text `#cdd6f4`, muted `#9399b2`, border `#313244`, accent `#cba6f7`, interactive `#fab387`, error `#f38ba8`, success `#a6e3a1`, selection `#45475a`, action_dim `#313244`;
diff +/− `#a6e3a1`/`#f38ba8` bg `#24312b`/`#3c2a32` gutter `#1e2a25`/`#32232a` ctx `#181825`/`#9399b2`;
md heading `#cba6f7`, strong `#fab387`, emphasis/quote `#f9e2af`, code `#a6e3a1`, list/link `#89b4fa`, link_label `#89dceb`, rule `#a6adc8`;
syn comment `#9399b2`, string `#a6e3a1`, number `#fab387`, keyword `#cba6f7`, function `#89b4fa`, type `#f9e2af`, variable `#f38ba8`, operator `#89dceb`.

### dracula (json confirmed)
defs: background `#282a36`, currentLine/selection `#44475a`, foreground `#f8f8f2`, comment `#6272a4`, cyan `#8be9fd`, green `#50fa7b`, orange `#ffb86c`, pink `#ff79c6`, purple `#bd93f9`, red `#ff5555`, yellow `#f1fa8c`.
sentuhan: raised `#21222c`, bg `#282a36`, text `#f8f8f2`, muted `#6272a4`, border `#44475a`, accent `#bd93f9`, interactive `#ff79c6`, error `#ff5555`, success `#50fa7b`, selection `#44475a`, action_dim `#343746`;
diff +/− `#50fa7b`/`#ff5555`, diffHighlightAdded green / diffHighlightRemoved red, bg `#1a3a1a`/`#3a1a1a` (diffAddedBg/diffRemovedBg), gutter add/rem `#1a3a1a`/`#3a1a1a` (diffAddedLineNumberBg/diffRemovedLineNumberBg), ctx_gutter `#21222c` (diffContextBg), ctx_text `#6272a4` (diffContext);
md heading purple `#bd93f9`, strong orange `#ffb86c`, emphasis yellow `#f1fa8c`, quote/rule comment `#6272a4`, code green `#50fa7b`, list purple `#bd93f9`, link cyan `#8be9fd`, link_label pink `#ff79c6`;
syn comment `#6272a4`, string `#f1fa8c`, number `#bd93f9`, keyword `#ff79c6`, function `#50fa7b`, type `#8be9fd`, variable `#f8f8f2`, operator `#ff79c6`.

### gruvbox (json confirmed)
defs dark: darkBg0 `#282828`, darkBg1 `#3c3836`, darkBg2 `#504945`, darkBg3 `#665c54`, darkFg0 `#fbf1c7`, darkFg1 `#ebdbb2`, darkGray `#928374`, darkGreen `#98971a`, darkGreenBright `#b8bb26`, darkRed `#cc241d`, darkRedBright `#fb4934`, darkYellowBright `#fabd2f`, darkBlueBright `#83a598`, darkPurpleBright `#d3869b`, darkAquaBright `#8ec07c`, darkOrangeBright `#fe8019`. light: lightGray `#7c6f64`, lightRed `#9d0006`, lightGreen `#79740e`, lightYellow `#b57614`, lightBlue `#076678`, lightPurple `#8f3f71`, lightAqua `#427b58`, lightOrange `#af3a03`.
theme: base `#282828`, raised `#3c3836`, text `#ebdbb2`, muted `#928374`, border `#665c54`, accent `#fe8019`, interactive `#fabd2f`, error `#fb4934`, success `#b8bb26`, selection `#504945`, action_dim `#3c3836`;
diff fg `#98971a`/`#cc241d`, bg `#32302f`/`#322929`, gutter `#2a2827`/`#2a2222` (diffAddedLineNumberBg/diffRemovedLineNumberBg), ctx_gutter `#3c3836` (diffContextBg), ctx_text `#928374` (diffContext);
md heading `#83a598` (darkBlueBright), strong `#fe8019`, emphasis `#d3869b`, quote/rule `#928374`, code `#fabd2f`, list `#83a598`, link `#8ec07c` (darkAquaBright), link_label `#b8bb26` (darkGreenBright);
syn comment `#928374`, string `#fabd2f`, number `#d3869b`, keyword `#fb4934`, function `#b8bb26`, type `#8ec07c`, variable `#83a598`, operator `#fe8019`.

### nord (json confirmed)
defs nord0 `#2E3440` … nord15 `#B48EAD` (nord2 `#434C5E`, nord3 `#4C566A`, nord6 `#ECEFF4`, nord7 `#8FBCBB`, nord8 `#88C0D0`, nord9 `#81A1C1`, nord10 `#5E81AC`, nord11 `#BF616A`, nord12 `#D08770`, nord13 `#EBCB8B`, nord14 `#A3BE8C`, nord15 `#B48EAD`).
theme: base `#2e3440`, raised `#3b4252`, text `#eceff4`, muted `#8b95a7`, border `#434c5e`, accent `#88c0d0`, interactive `#8fbcbb`, error `#bf616a`, success `#a3be8c`, selection `#4c566a`, action_dim `#3b4252`;
diff +/− `#a3be8c`/`#bf616a`, bg `#3b4252`/`#3b4252`, gutter `#3b4252`, ctx_gutter `#3b4252` (diffContextBg=nord1), ctx_text `#8b95a7`;
md heading `#88c0d0`, strong `#ebcb8b`, emphasis `#d08770`, quote/rule `#8b95a7`, code `#a3be8c`, list `#88c0d0`, link `#81a1c1`, link_label `#8fbcbb`;
syn comment `#8b95a7`, string `#a3be8c`, number `#b48ead`, keyword `#81a1c1`, function `#88c0d0`, type `#8fbcbb`, variable `#8fbcbb`, operator `#81a1c1`.

### tokyonight (json confirmed; accent deviasi per formula)
defs dark: darkStep1 `#1a1b26`, darkStep2 `#1e2030`, darkStep3 `#222436`, darkStep4 `#292e42`, darkStep5 `#3b4261`, darkStep6 `#545c7e`, darkStep7 `#737aa2`, darkStep8 `#9099b2`, darkStep9 `#82aaff`, darkStep10 `#89b4fa`, darkStep11 `#828bb8`, darkStep12 `#c8d3f5`, darkRed `#ff757f`, darkOrange `#ff966c`, darkYellow `#ffc777`, darkGreen `#c3e88d`, darkCyan `#86e1fc`, darkPurple `#c099ff`.
json theme: primary=darkStep9, secondary=darkPurple, accent=darkOrange, error=darkRed, success=darkGreen, text=darkStep12, textMuted=darkStep11, background=darkStep1, backgroundPanel=darkStep2, backgroundElement=darkStep3, border=darkStep7, borderActive=darkStep8, borderSubtle=darkStep6.
Keputusan: accent `#82aaff` (signature; deviasi dari json accent=darkOrange `#ff966c`), interactive `#c099ff`; action_dim `#292e42` (derived dim surface).
diff fg `#4fd6be`/`#c53b53`, highlightAdded `#b8db87`/highlightRemoved `#e26a75`, bg `#20303b`/`#37222c`, gutter `#1b2b34`/`#2d1f26` (diffAddedLineNumberBg/diffRemovedLineNumberBg), ctx_gutter `#1e2030` (diffContextBg=darkStep2), ctx_text `#828bb8`;
md heading `#c099ff`, strong `#ff966c`, emphasis/quote `#ffc777`, code `#c3e88d`, list/link `#82aaff`, link_label `#86e1fc`, rule `#828bb8`;
syn comment `#828bb8`, string `#c3e88d`, number `#ff966c`, keyword `#c099ff`, function `#82aaff`, type `#ffc777`, variable `#ff757f`, operator `#86e1fc`.
