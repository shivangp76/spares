//! The order notes are learned in: each note's *position*, which the queue's link holds and its
//! created orders follow (see [`QueueConfig::order_by_links`]).
//!
//! Notes come in the order they were created, except that a note's prerequisites come before it.
//! Notes added one after another often build on each other without linking, so the order keeps
//! as much of the created order as the links allow:
//!
//! - A prerequisite that is newer than a note it is a prerequisite of moves forward, to just
//!   before the first note that needs it, and its own prerequisites come with it. The notes that
//!   build on it stay where they were, among the notes added around them.
//! - Notes that link to each other in a loop, directly or through other notes, can't all come
//!   after their prerequisites. Within such a loop, only the links to older notes count, as they
//!   all did before positions; links into and out of the loop all count. `in_link_cycle` searches
//!   list the notes of loops, whose links are often "see also" links worth making
//!   `rel: "related"`.
//!
//! For example, with notes created in the order *warp*, *latency hiding*, *thread hierarchy*,
//! *memory hierarchy*, where *warp* builds on *thread hierarchy* and *latency hiding* on *warp*
//! and *memory hierarchy*, the order is *thread hierarchy*, *warp*, *memory hierarchy*,
//! *latency hiding*.
//!
//! The order is worked out by placing the notes from the last: each time, the newest note that no
//! note still to be placed builds on. That puts every note as late as its dependents allow, which
//! is what pulls a prerequisite forward only as far as it has to go.
//!
//! Positions are kept in the `note_position` table, worked out again by [`ensure_note_positions`]
//! when a note is added or deleted, its `created_at` changes, a link changes (triggers replace the
//! token in `note_position_version`), or the settings they were worked out with change. Like
//! links, they are not synced: each machine orders notes by its own links. They take about 15 ms
//! for 5,000 notes.

use std::collections::BinaryHeap;

use itertools::Itertools;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use crate::Error;
use crate::config::QueueConfig;
use crate::model::NoteId;

/// The SQL of the position of the note aliased `note`, or NULL if it has none yet.
pub(crate) fn position_sql(note: &str) -> String {
    format!("(SELECT np.position FROM note_position np WHERE np.note_id = {note}.id)")
}

/// The SQL of whether the note aliased `note` is in a loop of links, see the
/// [module docs](self).
pub(crate) fn in_link_cycle_sql(note: &str) -> String {
    format!(
        "COALESCE((SELECT np.in_link_cycle FROM note_position np WHERE np.note_id = {note}.id), 0)"
    )
}

/// The order of notes `0..count`, which are numbered in the order they were created, given that
/// `prerequisites` lists each `(note, prerequisite)` pair: the notes, first to last, and for each
/// note whether it is in a loop of links. See the [module docs](self).
pub(crate) fn order_notes(
    count: usize,
    prerequisites: &[(usize, usize)],
) -> (Vec<usize>, Vec<bool>) {
    let mut builds_on = vec![Vec::new(); count];
    for &(note, prerequisite) in prerequisites.iter().unique() {
        if note != prerequisite && note < count && prerequisite < count {
            builds_on[note].push(prerequisite);
        }
    }
    let component = strongly_connected(&builds_on);
    let mut sizes = vec![0_usize; count];
    for &c in &component {
        sizes[c] += 1;
    }
    let in_cycle = component.iter().map(|&c| sizes[c] > 1).collect::<Vec<_>>();
    // Inside a loop, only links to older notes count, which leaves no loop.
    for (note, prerequisites) in builds_on.iter_mut().enumerate() {
        prerequisites.retain(|&prerequisite| {
            component[prerequisite] != component[note] || prerequisite < note
        });
    }
    let mut dependents_left = vec![0_usize; count];
    for prerequisites in &builds_on {
        for &prerequisite in prerequisites {
            dependents_left[prerequisite] += 1;
        }
    }
    // The newest note nothing left builds on goes last.
    let mut ready = (0..count)
        .filter(|&note| dependents_left[note] == 0)
        .collect::<BinaryHeap<_>>();
    let mut order = Vec::with_capacity(count);
    while let Some(note) = ready.pop() {
        order.push(note);
        for &prerequisite in &builds_on[note] {
            dependents_left[prerequisite] -= 1;
            if dependents_left[prerequisite] == 0 {
                ready.push(prerequisite);
            }
        }
    }
    order.reverse();
    (order, in_cycle)
}

/// The strongly connected component of each node of the graph whose edges are `edges`, by
/// Tarjan's algorithm, without recursion, so a long chain of links can't overflow the stack.
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let count = edges.len();
    let mut index = vec![UNSEEN; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut component = vec![UNSEEN; count];
    let mut next = 0;
    for root in 0..count {
        if index[root] != UNSEEN {
            continue;
        }
        // Each node being visited, with how many of its edges it has followed
        let mut visiting = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (node, ref mut followed)) = visiting.last_mut() {
            if let Some(&to) = edges[node].get(*followed) {
                *followed += 1;
                if index[to] == UNSEEN {
                    index[to] = next;
                    low[to] = next;
                    next += 1;
                    stack.push(to);
                    on_stack[to] = true;
                    visiting.push((to, 0));
                } else if on_stack[to] {
                    low[node] = low[node].min(index[to]);
                }
                continue;
            }
            visiting.pop();
            if let Some(&(parent, _)) = visiting.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component[member] = node;
                    if member == node {
                        break;
                    }
                }
            }
        }
    }
    component
}

/// What positions depend on besides notes and links, stored with them so that changing it works
/// them out again.
fn settings(queue: &QueueConfig) -> String {
    format!(
        "order_by_links={} linked_note_max_score={}",
        queue.order_by_links, queue.linked_note_max_score
    )
}

/// Works out the positions of notes again if anything they depend on changed since they last
/// were. Anything reading `note_position` calls this first.
pub(crate) async fn ensure_note_positions(
    conn: &mut SqliteConnection,
    queue: &QueueConfig,
) -> Result<(), Error> {
    let error = |source| Error::Sqlx { source };
    let settings = settings(queue);
    let current: bool = sqlx::query_scalar(
        "SELECT built_token IS token AND built_with = ? FROM note_position_version",
    )
    .bind(&settings)
    .fetch_one(&mut *conn)
    .await
    .map_err(error)?;
    if current {
        return Ok(());
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(error)?;
    // Writing first takes the write lock, waiting for other writers, before anything is read, so
    // the token read is the one the notes and links read go with.
    sqlx::query("DELETE FROM note_position")
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    let token: Vec<u8> = sqlx::query_scalar("SELECT token FROM note_position_version")
        .fetch_one(&mut *tx)
        .await
        .map_err(error)?;
    let notes: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note ORDER BY created_at, id")
        .fetch_all(&mut *tx)
        .await
        .map_err(error)?;
    let index = notes
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect::<std::collections::HashMap<_, _>>();
    let links: Vec<(NoteId, NoteId)> = sqlx::query_as(
        "SELECT DISTINCT note_id, prerequisite_id FROM note_prerequisite WHERE score <= ?",
    )
    .bind(queue.linked_note_max_score)
    .fetch_all(&mut *tx)
    .await
    .map_err(error)?;
    let prerequisites = links
        .iter()
        .filter_map(|(note, prerequisite)| Some((*index.get(note)?, *index.get(prerequisite)?)))
        .collect::<Vec<_>>();
    let (mut order, in_cycle) = order_notes(notes.len(), &prerequisites);
    if !queue.order_by_links {
        order = (0..notes.len()).collect();
    }
    // 3 variables a row, well under SQLite's limit of 32,766
    let rows = order.into_iter().enumerate().collect::<Vec<_>>();
    for chunk in rows.chunks(5000) {
        let sql = format!(
            "INSERT INTO note_position (note_id, position, in_link_cycle) VALUES {}",
            crate::api::placeholders_2d(chunk.len(), 3)
        );
        let mut insert = sqlx::query(sqlx::AssertSqlSafe(sql));
        for &(position, note) in chunk {
            insert = insert
                .bind(notes[note])
                .bind(i64::try_from(position).unwrap_or(i64::MAX))
                .bind(in_cycle[note]);
        }
        insert.execute(&mut *tx).await.map_err(error)?;
    }
    sqlx::query("UPDATE note_position_version SET built_token = ?, built_with = ?")
        .bind(token)
        .bind(&settings)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    tx.commit().await.map_err(error)
}

/// [`ensure_note_positions`] on a connection of `db`.
pub(crate) async fn ensure_note_positions_in(
    db: &SqlitePool,
    queue: &QueueConfig,
) -> Result<(), Error> {
    let mut conn = db.acquire().await.map_err(|e| Error::Sqlx { source: e })?;
    ensure_note_positions(&mut conn, queue).await
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::Utc;

    use super::*;
    use crate::api::queue::tests::create_cards;
    use crate::model::CardId;

    /// The notes of `cards`, in the order of their positions, after working them out.
    async fn positioned(pool: &SqlitePool, queue: &QueueConfig, cards: &[CardId]) -> Vec<CardId> {
        ensure_note_positions_in(pool, queue).await.unwrap();
        let ordered: Vec<CardId> = sqlx::query_scalar(
            "SELECT c.id FROM card c JOIN note_position np ON np.note_id = c.note_id \
             ORDER BY np.position",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        ordered
            .into_iter()
            .filter(|id| cards.contains(id))
            .collect()
    }

    async fn link(pool: &SqlitePool, from: CardId, to: CardId, score: f64) {
        sqlx::query(
            r#"INSERT INTO note_link
                (parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score, relation)
            VALUES ((SELECT note_id FROM card WHERE id = ?), (SELECT note_id FROM card WHERE id = ?),
                0, 'keyword', 'keyword', ?, 0)"#,
        )
        .bind(from)
        .bind(to)
        .bind(score)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Positions are worked out again when a link, a note or a setting they depend on changes,
    /// and only then.
    #[sqlx::test]
    async fn positions_follow_links_notes_and_settings(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b, c] = create_cards(&pool, 3, now - Duration::days(1)).await[..] else {
            unreachable!()
        };
        let queue = QueueConfig::default();
        assert_eq!(positioned(&pool, &queue, &[a, b, c]).await, [a, b, c]);
        link(&pool, a, c, 1.0).await;
        assert_eq!(positioned(&pool, &queue, &[a, b, c]).await, [c, a, b]);
        let built: Vec<u8> = sqlx::query_scalar("SELECT built_token FROM note_position_version")
            .fetch_one(&pool)
            .await
            .unwrap();
        ensure_note_positions_in(&pool, &queue).await.unwrap();
        let again: Vec<u8> = sqlx::query_scalar("SELECT built_token FROM note_position_version")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(built, again, "nothing changed, so nothing was worked out");
        // A stricter score leaves the link out, and the setting off orders by age alone
        let strict = QueueConfig {
            linked_note_max_score: 0.5,
            ..queue.clone()
        };
        assert_eq!(positioned(&pool, &strict, &[a, b, c]).await, [a, b, c]);
        assert_eq!(positioned(&pool, &queue, &[a, b, c]).await, [c, a, b]);
        let by_age = QueueConfig {
            order_by_links: false,
            ..queue.clone()
        };
        assert_eq!(positioned(&pool, &by_age, &[a, b, c]).await, [a, b, c]);
        // `b` becoming the oldest note moves it first
        let note_sql =
            |sql: &str| format!("{sql} WHERE id = (SELECT note_id FROM card WHERE id = ?)");
        sqlx::query(sqlx::AssertSqlSafe(note_sql(
            "UPDATE note SET created_at = created_at - 86400",
        )))
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(positioned(&pool, &queue, &[a, b, c]).await, [b, c, a]);
        // A new note makes them out of date
        sqlx::query(sqlx::AssertSqlSafe(note_sql(
            "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id, usn) \
             SELECT id + 1000, data, created_at, updated_at, custom_data, parser_id, -1 FROM note",
        )))
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
        let current: bool =
            sqlx::query_scalar("SELECT built_token IS token FROM note_position_version")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!current);
        // Deleting `c` lets `a` go back to its place
        sqlx::query(sqlx::AssertSqlSafe(note_sql("DELETE FROM note")))
            .bind(c)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(positioned(&pool, &queue, &[a, b]).await, [b, a]);
    }

    fn order(count: usize, prerequisites: &[(usize, usize)]) -> Vec<usize> {
        order_notes(count, prerequisites).0
    }

    /// `position` sorts and compares notes by their positions, and `in_link_cycle` finds the
    /// notes of loops, working positions out first.
    #[sqlx::test]
    async fn searches_read_positions(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b, c, d] = create_cards(&pool, 4, now - Duration::days(1)).await[..] else {
            unreachable!()
        };
        // `a` builds on `c`, and `b` and `d` on each other
        link(&pool, a, c, 0.0).await;
        link(&pool, b, d, 0.0).await;
        link(&pool, d, b, 0.0).await;
        let search = async |query: &str| {
            crate::search::evaluator::Evaluator::new(query)
                .get_card_ids(&pool)
                .await
                .unwrap_or_else(|e| panic!("{query}: {e}"))
        };
        assert_eq!(search("sort_by_asc=position").await, [c, a, b, d]);
        assert_eq!(search("sort_by_desc=c.id,position").await, [d, c, b, a]);
        assert_eq!(search("position<2 sort_by_asc=position").await, [c, a]);
        assert_eq!(search("in_link_cycle sort_by_asc=c.id").await, [b, d]);
        assert_eq!(search("-in_link_cycle sort_by_asc=c.id").await, [a, c]);
        assert_eq!(search("in_link_cycle=false sort_by_asc=c.id").await, [a, c]);
    }

    #[test]
    fn notes_without_links_keep_the_created_order() {
        assert_eq!(order(4, &[]), [0, 1, 2, 3]);
        // Links to older notes change nothing either
        assert_eq!(order(4, &[(1, 0), (3, 1), (2, 0)]), [0, 1, 2, 3]);
    }

    /// A newer prerequisite moves to just before the first note needing it, with its own
    /// prerequisites, and the notes around stay in order.
    #[test]
    fn a_newer_prerequisite_comes_just_before_its_note() {
        // 1 builds on 3, which builds on 4
        assert_eq!(order(6, &[(1, 3), (3, 4)]), [0, 4, 3, 1, 2, 5]);
        // The module docs' example: warp (0) on thread hierarchy (2), latency hiding (1) on warp
        // and memory hierarchy (3)
        assert_eq!(order(4, &[(0, 2), (1, 0), (1, 3)]), [2, 0, 3, 1]);
        // Two notes needing one prerequisite: it comes before the first of them
        assert_eq!(order(4, &[(1, 3), (2, 3)]), [0, 3, 1, 2]);
    }

    #[test]
    fn every_note_comes_after_its_prerequisites() {
        let links = [
            (0, 5),
            (2, 7),
            (7, 1),
            (4, 6),
            (6, 3),
            (3, 8),
            (1, 0),
            (9, 2),
        ];
        let order = order(10, &links);
        let position = |note: usize| order.iter().position(|&n| n == note).unwrap();
        for (note, prerequisite) in links {
            assert!(
                position(prerequisite) < position(note),
                "{prerequisite} before {note}"
            );
        }
        assert_eq!(
            order.iter().sorted().copied().collect_vec(),
            (0..10).collect_vec()
        );
    }

    /// In a loop, only links to older notes count; links into and out of it still do.
    #[test]
    fn loops_fall_back_to_the_created_order() {
        // 1 and 2 build on each other, and 0 builds on 2
        let (order, in_cycle) = order_notes(4, &[(1, 2), (2, 1), (0, 2)]);
        assert_eq!(in_cycle, [false, true, true, false]);
        assert_eq!(order, [1, 2, 0, 3]);
        // A note linking to itself is no loop
        let (order, in_cycle) = order_notes(2, &[(1, 1), (0, 0)]);
        assert_eq!((order, in_cycle), (vec![0, 1], vec![false, false]));
    }

    #[test]
    fn long_chains_do_not_overflow_the_stack() {
        let count = 200_000;
        // Each note builds on the next, and the last on the first
        let mut links = (0..count - 1).map(|note| (note, note + 1)).collect_vec();
        assert_eq!(order(count, &links), (0..count).rev().collect_vec());
        links.push((count - 1, 0));
        let (order, in_cycle) = order_notes(count, &links);
        assert!(in_cycle.iter().all(|&c| c));
        assert_eq!(order, (0..count).collect_vec());
    }
}
