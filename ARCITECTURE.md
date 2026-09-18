Phase 1: Foundation Setup
└── Cargo setup + Tokio event bus + Ratatui Terminal init / cleanup handlers

Phase 2: Async Directory Engine
└── Non-blocking directory streaming via channels + cancellation tokens for fast scrolling

Phase 3: Miller Columns UI Layout
└── Parent, Current, and Preview pane state rendering in Ratatui

Phase 4: Advanced Previews & Integrations
└── Inline images (Kitty/Sixel), Git status overlays, and file system watching (`notify`)

Phase 5: Performance & Profiling
└── Cargo release profile tuning, Flamegraph profiling, zero-copy optimization