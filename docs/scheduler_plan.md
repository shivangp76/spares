# Scheduler plan

Work left to make spares work with any scheduler, not just FSRS. What is done is documented on the
`SrsScheduler` trait in `spares_core::schedulers` (`cargo doc --workspace --no-deps
--document-private-items --open`): ratings, `is_pass` and the lapses counted from it, and
`retrievability` and `retrievability_key`.

Each improvement below says what is missing, when it matters (use case), a small example, and how
it could be built.

## States declared by the scheduler

Let each scheduler list its card states, and say what spares should treat each one as, rather than
everything using FSRS's four.

- **Use case.** `Card::state` is documented as relative to the card's scheduler, but the queue,
  daily limits, sibling burying, statistics, search and both card info views all use FSRS's states:
  `review_state()`, `learning_states()` and `relearning_state()` are FSRS's, re-exported from
  `schedulers/mod.rs`, and search (`CARD_STATES`), the CLI (`REVIEW_STATE` in its tests) and the
  web app (`STATE_LABELS`) write out 0 to 3 again. A scheduler with a state FSRS lacks has nowhere
  to put it, and one whose state ids mean something else is misread everywhere.
- **Example.** A scheduler that gives well known cards a `mastered` state (id 4), reviewed rarely
  and counted apart in statistics. Today such a card is queued with the reviews only because it is
  not a learning state, but it does not count toward the review limit, statistics count its reviews
  as learning ones, and `c.state=mastered` is an error. With it declared as a state with the
  role of a review, it is queued, limited and buried like a review, statistics count it as one, and
  search and card info show its name.
- **How.**
  - The trait lists the states, each with the role spares' own code needs:

    ```rust
    pub enum StateRole { New, Learning, Review, Relearning }
    pub struct CardState { pub id: StateId, pub name: &'static str, pub role: StateRole }
    fn get_states(&self) -> Vec<CardState>;
    ```

    FSRS returns its four states, each with its own role.
  - Code that asks for one state asks for a role instead, as a set of ids from every scheduler:
    `states_with_role(StateRole::Review)` in place of `review_state()`, used in SQL as
    `c.state IN (...)`. The places that compare with a single id need the most care: the class SQL
    in `api/statistics/graphs.rs`, the counts by state in `api/preset.rs` and
    `api/offline_review.rs`, the groups and linked-note SQL in `api/queue.rs`, sibling burying and
    the studied counts in `api/review.rs`, and `lapse_sql` and `get_leeches`.
  - Two schedulers may give the same id only to states with the same role, which a test over
    `get_all_schedulers` checks. Otherwise the queue would need each card's scheduler in every
    query, which is a lookup in `review_log` per card. `NEW_CARD_STATE` (0) stays shared with the
    role `New`, since every card starts there whatever will schedule it.
  - Names come from the schedulers too: search's `c.state=<name>` looks the name up among every
    scheduler's states, and the CLI and web app fetch them from a new
    `GET /api/scheduler/{name}/states`, as they fetch ratings, instead of `STATE_LABELS`.
  - The `Card::state` doc then says the state is one of its scheduler's `get_states`.
