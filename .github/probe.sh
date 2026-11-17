# Temporary probe (macos2).
set -x
cargo test --lib --all-features probe_macos_fonts -- --nocapture 2>&1 | grep -E "PROBE|panicked|test result"
cargo test --lib --all-features lines_that_run_both_ways_are_drawn_where_they_are_seen syllables_are_put_in_the_order 2>&1 | tail -30
ls /System/Library/Fonts/ | head -100
ls /System/Library/Fonts/Supplemental | grep -i -E "thai|thon|ayut|silom|krung|sukh|geeza|arab|baghdad|nadeem" 
