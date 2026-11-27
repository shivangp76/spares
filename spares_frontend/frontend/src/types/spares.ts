/** The session with the host that serves the web app (`hostUrl`). */
export interface Credentials {
  /**
   * The session's CSRF token, which every request sends in `X-CSRF-Token`. The session itself is
   * a cookie the page can't read (see `spares_server::host::auth`).
   */
  csrfToken: string;
  /** Stable host collection ID. */
  accountId: string;
  /** The account's name */
  user: string;
  /** Picked with the host on logging in, and changed on the Settings page (`chooseScheduler`). */
  schedulerName: string;
}

export interface GetReviewCardResponse {
  note_id: number;
  card_order: number;
  card_id: number;
  card_state: number;
  card_front_rendered_path: string;
  // Externally-tagged serde enum: { "CardBack": "path" } | { "Note": "path" }
  card_back_rendered_path: { CardBack: string } | { Note: string };
  // HTML shown instead of the rendered files. Present iff both sides have it (Markdown)
  card_front_web_path?: string;
  /** Rendered HTML included with a review selection when the host can send it inline. */
  front_html?: string;
  card_back_web_path?: { CardBack: string } | { Note: string };
  card_front_raw_path: string;
  card_back_raw_path: { CardBack: string } | { Note: string };
  note_raw_path: string;
  // When the note was last changed
  note_updated_at: string;
  parser_name: string;
  // The note's keywords and tags, shown beside the card as its rendered files leave them out
  keywords: string[];
  // In name order
  tags: string[];
  // The card's own tags, which its note doesn't have, in name order
  card_tags: string[];
  // Present iff the card is reviewed by running an external command (CLI only)
  cli?: CliReviewInfo;
  // Present iff the parser's sources are compiled in the browser (Typst)
  browser_sources?: BrowserRenderSources;
  // Present iff the card is a Markdown card on a device that keeps a copy, which shows them while the server hasn't rendered its note's latest change
  device_sources?: BrowserRenderSources;
  // Present iff the card asks for its answers to be typed (the cloze key `t`)
  typed_answer?: TypedAnswerInfo;
  // Present iff the card has audio clips or text to read aloud
  audio?: CardAudio;
  // Present iff the card's back still masks some of its image occlusions: those, which the review page shows without masks
  image_occlusions?: UnmaskedImageOcclusion[];
  cards_left_by_state: Record<string, number>;
  time_estimate: number; // seconds (DurationSeconds<i64>)
  linked_notes: ReviewLinkedNote[];
}

/** An image occlusion of a review card whose back masks some of it. */
export interface UnmaskedImageOcclusion {
  /** As the card's figure is captioned */
  caption: string;
  /** Its place among the card's image occlusions, from 1 */
  order: number;
}

export interface CliReviewInfo {
  exec: string;
  surrounding: string;
}

// Internally-tagged serde enum
export type TypeAnswer =
  | { kind: "diff"; ignore_case: boolean; ignore_combining: boolean }
  | { kind: "manual" };

export interface TypedBlank {
  expected: string;
  hint?: string;
}

export interface TypedAnswerInfo {
  mode: TypeAnswer;
  // One per answer cloze, in reading order
  blanks: TypedBlank[];
}

// Text read aloud by the device's voices (see `spares_core/src/parsers/tts.rs`)
export interface SpeechItem {
  text: string;
  // Such as `fr_FR` or `fr-FR`
  lang?: string;
  // A multiplier of the voice's rate
  speed?: number;
  // The name of a voice
  voice?: string;
}

// An audio clip's absolute path, fetched from the render assets, or text to read aloud
export type AudioItem = string | SpeechItem;

// In the order they appear in the note
export interface CardAudio {
  // Played when the card is shown
  front: AudioItem[];
  // Played when it is flipped. Clips on the front are not repeated.
  back: AudioItem[];
}

export interface BrowserRenderSources {
  card_front: string;
  // The card back, or the whole note if the card's back is the note
  card_back: string;
}

export interface ReviewLinkedNote {
  searched_keyword: string;
  note_id: number;
  matched_keyword: string | null;
  note_raw_path: string;
  relation: LinkRelation;
}

/**
 * Which way a link points, from its `rel:`: the note with it builds on the linked note
 * (`prerequisite`, the default), the linked note builds on it (`dependent`), or neither does.
 */
export type LinkRelation = "prerequisite" | "dependent" | "related";

export interface Rating {
  id: number;
  description: string;
}

export interface RatingSubmission {
  card_id: number;
  rating: number;
  recall_duration: number; // seconds
  rate_duration: number; // seconds
  tag_id: number | null;
}

// Externally-tagged serde enum
export type StudyAction =
  | { Rate: RatingSubmission }
  | { Bury: { card_id: number } }
  | { Advance: { count: number; query: string | null } }
  | { Postpone: { count: number; query: string | null } };

export interface SubmitStudyActionRequest {
  scheduler_name: string;
  action: StudyAction;
  /**
   * Names a rating or bury, so that sending it again (with the same request) carries it out once:
   * a retry gets the first attempt's response. Make one with `newActionId`.
   */
  action_id?: string;
  next_review?: { filter?: ReviewFilter; include_front_html?: boolean };
}

export interface SubmitStudyActionResponse {
  event_id: number | null;
  next_review?: { card: GetReviewCardResponse | null };
}

// Externally-tagged serde enum
export type ReviewFilter =
  { Query: string } | { FilteredTag: { tag_id: number } };

export interface QueuePositionRequest {
  // `null` for the default review
  filter: ReviewFilter | null;
  // `null` lists the whole queue
  match_query: string | null;
  // The most cards to list in `positions` and `not_shown`, or `null` for all
  limit: number | null;
}

export interface QueueCard {
  card_id: number;
  note_id: number;
  card_order: number;
  card_state: number;
  due: string; // ISO 8601
  // The start of the note's text, on one line, cut at 100 characters with `…`
  note_preview: string;
}

export interface QueuePosition extends QueueCard {
  // From 1, for the card review shows next
  position: number;
}

export interface HeldBackCard extends QueueCard {
  // The earliest review can show the card (ISO 8601)
  shown_from: string;
}

// Externally-tagged serde enum
export type NotShownReason =
  | "Suspended"
  | "Buried"
  | "NotDue"
  | "WaitingOnLinkedNote"
  | { NewLimit: { preset: string | null } }
  | { ReviewLimit: { preset: string | null } };

export interface NotShownCard extends QueueCard {
  reason: NotShownReason;
}

export interface QueuePositionResponse {
  // How many cards the review would show now
  total: number;
  // How many of them match the query: `total` when there is none
  matching: number;
  positions: QueuePosition[];
  // Learning cards held back by the learn ahead limit, soonest first
  held_back: HeldBackCard[];
  not_shown_total: number;
  not_shown: NotShownCard[];
  // Matching cards that are not in the review's filter at all
  not_in_review: number;
}

export interface ReviewSnapshotResponse {
  tag_id: number;
  tag_name: string;
  // `false` when an existing snapshot built earlier on the same day was resumed
  rebuilt: boolean;
  card_count: number;
}

export interface ReviewConfig {
  flagged_tag_name: string;
  set_card_due_date_duration: number; // seconds
  // How long a timebox lasts, in seconds (`queue.timebox`)
  timebox: number | null;
  // `queue.auto_advance`
  auto_advance: AutoAdvance | null;
}

/** `[queue.auto_advance]`: when review shows the answer and acts on its own. */
export interface AutoAdvance {
  // Seconds, 0 to wait for the answer to be shown by hand
  show_answer_after: number;
  // Seconds, 0 to wait for a rating by hand
  rate_after: number;
  action: "again" | "hard" | "good" | "bury";
}

export interface TagResponse {
  id: number;
  name: string;
  description: string;
  // Present iff the tag is a filtered tag, whose membership is defined by the query
  query: string | null;
  auto_delete: boolean;
  // For a filtered tag: whether rating a card while studying it reschedules the card
  reschedule: boolean;
  created_at: number; // unix seconds
  updated_at: number; // unix seconds
}

export type TagSelector = { Id: number } | { Name: string };

export interface CreateTagRequest {
  name: string;
  description: string;
  query: string | null;
  auto_delete: boolean;
  reschedule?: boolean;
}

export interface UpdateTagRequest {
  tag_to_modify: TagSelector;
  name?: string;
  description?: string;
  // Omit to leave unchanged, `null` to clear
  query?: string | null;
  auto_delete?: boolean;
  reschedule?: boolean;
}

export interface ParserResponse {
  id: number;
  name: string;
}

export type SpecialState =
  "Suspended" | "UserBuried" | "SchedulerBuried" | "BuriedUntilLaterToday";
export type SpecialStateUpdate =
  "Suspended" | "Buried" | "BuriedUntilLaterToday";

export interface CardResponse {
  id: number;
  note_id: number;
  order: number;
  created_at: string;
  updated_at: string;
  due: string;
  stability: number;
  difficulty: number;
  desired_retention: number;
  special_state: SpecialState | null;
  state: number;
  custom_data: unknown;
}

export type CardsSelector = { Ids: number[] } | { Query: string };

export interface UpdateCardsRequest {
  selector: CardsSelector;
  desired_retention?: number;
  // Omit to leave unchanged, `null` to clear
  special_state?: SpecialStateUpdate | null;
  due?: string;
  // The cards' own tags, which their notes don't have. Omit to leave them unchanged.
  tags?: UpdateTags;
}

/** How a request changes tags, as `UpdateTags` in spares_core takes it */
export type UpdateTags =
  | {
      ModifyTags: {
        tags_to_remove: string[] | null;
        tags_to_add: string[] | null;
      };
    }
  | { SetTags: string[] };

export interface UpdateCardsResponse {
  cards: CardResponse[];
  event_id: number | null;
}

export interface ForgetCardResponse {
  card: CardResponse;
  event_id: number | null;
}

/** One row of a card's review log, with the memory state the card had after it. */
export interface CardReviewEntry {
  id: number;
  /** `Cram` is a rating in a filtered tag that does not reschedule, which left the card's schedule alone. `Reschedule` is a postponement or advance, which moved the due date without a review. */
  kind: "Review" | "Forget" | "Cram" | "Reschedule";
  reviewed_at: string;
  /** Relative to `scheduler_name`. */
  rating: number | null;
  scheduler_name: string;
  previous_state: number;
  scheduled_time: number | null; // seconds
  recall_duration: number | null; // seconds
  rate_duration: number | null; // seconds
  tag_id: number | null;
  /** Replayed with the scheduler's current parameters. `null` after a forget. A cram keeps the one before it. */
  stability: number | null;
  difficulty: number | null;
  /** Whether this review was a lapse: failed, by its scheduler's ratings, after the card was learned. */
  lapse: boolean;
  /** For a `Reschedule`, the due dates before and after, in Unix seconds: a postponement if `due` is later. */
  reschedule: { previous_due: number; due: number } | null;
}

/** An event that undo or redo reversed. */
export interface EventSummary {
  id: number;
  /** The action the user originally took, e.g. "Rate card 3 (Good)", whether undone or redone. */
  description: string;
}

/** `spares_core::schema::undo::EventEntry`: an event of the history, or a group of them, which undo and redo revert together. */
export interface EventEntry {
  /** The event's, or the group's first event's: the one to undo or redo it by */
  id: number;
  /** `spares_core::model::EventType`, e.g. `CreateNotes` */
  kind: string;
  action: "Do" | "Undo" | "Redo";
  created_at: string;
  /** For a group, by its note events, with how many other changes it made */
  description: string;
  group_size: number;
  /** All of it was undone (or redone, for an undo) */
  reverted: boolean;
  /** Made by spares itself, like the daily unbury; a plain undo skips it */
  automatic: boolean;
}

export interface UndoEventResponse {
  undone_events: EventSummary[];
  /** The undo events that were appended, oldest first. Pass one to `redoEvent`. */
  undo_event_ids: number[];
}

export interface RedoEventResponse {
  redone_events: EventSummary[];
  /** The redo events that were appended, oldest first. Pass one to `undoEvent`. */
  redo_event_ids: number[];
}

/** What spares treats a card state as, whatever its scheduler calls it. */
export type StateRole = "New" | "Learning" | "Review" | "Relearning";

/** A state a scheduler can put a card in, from `getSchedulerStates`. */
export interface CardState {
  id: number;
  name: string;
  role: StateRole;
}

export interface StatisticsResponse {
  /** Distinct cards reviewed, crams excluded. */
  cards_studied_count: number;
  recall_duration: number;
  rate_duration: number;
  /** Ratings in filtered tags that do not reschedule, each rating counted, e.g. 120 after going twice through 60 cards. */
  cram_count: number;
  /** Seconds spent on those crams, recalling and rating together. */
  cram_duration: number;
  card_count_by_state: Record<string, number>;
  due_count_by_state: Record<string, number>;
  due_count_by_date: Record<string, number>;
  advance_safe_count: number;
  postpone_safe_count: number;
}

/** `POST /api/statistics/graphs`. Days and hours are local to `tz`. */
export interface StatisticsGraphsRequest {
  scheduler_name: string;
  /** Only the cards matching this search query; `null` is every card. */
  query: string | null;
  /** The days the reviews, hourly breakdown, answer buttons and added cards cover, today included; `null` is all time. */
  period_days: number | null;
  forecast_days: number;
  date: string;
  /** An IANA time zone, e.g. `Europe/Berlin`. */
  tz: string | null;
}

export interface TodayStats {
  reviews: number;
  passed: number;
  seconds: number;
  learning: number;
  relearning: number;
  young: number;
  mature: number;
  crams: number;
  crams_passed: number;
  cram_seconds: number;
}

export interface ForecastDay {
  date: string;
  learning: number;
  review: number;
}

export interface ReviewDay {
  date: string;
  learning: number;
  relearning: number;
  young: number;
  mature: number;
  cram: number;
  seconds: number;
}

export interface ReviewsSummary {
  reviews: number;
  crams: number;
  seconds: number;
  days_studied: number;
  days: number;
  daily_average: number;
}

export interface PassFail {
  passed: number;
  failed: number;
}

export interface RetentionRow {
  label: string;
  /** `null` for all time. */
  days: number | null;
  young: PassFail;
  mature: PassFail;
}

export interface AnswerGroup {
  /** `[rating id, count]` per rating of `AnswerButtons.ratings`, in its order. */
  counts: [number, number][];
  passed: number;
  total: number;
}

export interface AnswerButtons {
  ratings: Rating[];
  learning: AnswerGroup;
  young: AnswerGroup;
  mature: AnswerGroup;
}

export interface HourStats {
  reviews: number;
  passed: number;
  crams: number;
  crams_passed: number;
}

export interface CardCounts {
  /** Cards neither suspended nor buried, by state id. */
  by_state: Record<string, number>;
  young: number;
  mature: number;
  suspended: number;
  buried: number;
  total: number;
}

/** Bucket `i` holds the values from `edges[i]` to `edges[i + 1]`; the last bucket can be wider, holding the tail. */
export interface Histogram {
  edges: number[];
  counts: number[];
  mean: number | null;
}

export interface Distributions {
  interval: Histogram;
  stability: Histogram;
  difficulty: Histogram;
  /** `null` if the scheduler has no forgetting curve. */
  retrievability: Histogram | null;
  total_knowledge: number | null;
  /** Reviews a day the cards in review cost once the collection settles: the sum of 1 / max(1, interval in days). */
  daily_load: number;
  /** Over the cards in review, neither new, learning nor suspended. `null` without a forgetting curve or such cards. */
  average_retrievability: number | null;
  average_stability: number | null;
}

export interface StepsStatsRequest {
  /** `null` for the cards that use no preset. */
  preset: string | null;
  period_days: number | null;
}

export type StepsGroupKind =
  "again" | "hard" | "good" | "again_then_good" | "relearning";

export interface StepsGroup {
  kind: StepsGroupKind;
  reviews: number;
  recalled: number;
  /** Days. `null` when the reviews do not pin it down. */
  stability: number | null;
  /** Seconds until recall falls to the desired retention. */
  interval: number | null;
}

/** Seconds; `null` without enough reviews. */
export interface RecommendedSteps {
  first_learning_step: number | null;
  second_learning_step: number | null;
  relearning_step: number | null;
}

export interface StepsStatsResponse {
  presets: string[];
  desired_retention: number;
  min_reviews: number;
  groups: StepsGroup[];
  /** Seconds. */
  learning_steps: number[];
  relearning_steps: number[];
  recommended: RecommendedSteps;
}

/** `spares_core::schema::review::ParametersEvaluation`: lower is better for both. */
export interface ParametersEvaluation {
  log_loss: number;
  /** The RMSE between predicted and actual recall rates, as a fraction. */
  rmse_bins: number;
}

/** `spares_core::schema::review::OptimizeResponse` */
export interface OptimizeResponse {
  card_count: number;
  review_count: number;
  current_parameters: number[];
  current: ParametersEvaluation;
  optimized_parameters: number[];
  optimized: ParametersEvaluation;
  /** Whether the optimized parameters were saved and every card rescheduled. */
  applied: boolean;
  /** The event of saving the parameters and rescheduling, when `applied`. Undoing it puts back
      both the cards and the previous parameters. */
  event_id: number | null;
}

export type TableDelimiter = "tab" | "comma" | "semicolon" | "dash";

/** `spares_core::adapters::impls::delimited::Columns`, counted from 0 */
export interface TableColumns {
  front: number;
  back: number;
  tags: number | null;
}

/** `spares_core::adapters::impls::delimited::ImportTableRequest` */
export interface ImportTableRequest {
  text: string;
  delimiter: TableDelimiter | null;
  has_header: boolean | null;
  columns: TableColumns | null;
  parser: string;
  /** Whether the cells are plain text, whose markup characters are escaped */
  escape: boolean;
  tag: string | null;
  dry_run: boolean;
}

/** `spares_core::adapters::impls::delimited::TableNote` */
export interface TableNote {
  record: number;
  front: string;
  back: string;
  tags: string[];
  data: string;
}

/** `spares_core::adapters::impls::delimited::ImportTableResponse` */
export interface ImportTableResponse {
  delimiter: TableDelimiter;
  header: string[] | null;
  column_count: number;
  columns: TableColumns;
  note_count: number;
  preview: TableNote[];
  skipped_records: number[];
  event_id: number | null;
}

/** `spares_core::schema::review::OptimizeProposal`: parameters `auto_optimize` fitted, which
    predict the reviews better, waiting to be applied. */
export interface OptimizeProposal {
  parameters: number[];
  current_parameters: number[];
  current: ParametersEvaluation;
  optimized: ParametersEvaluation;
  review_count: number;
  card_count: number;
  fitted_at: string;
}

/** `spares_core::schema::review::OptimizeStatusResponse`: whether optimizing again is worth it. */
export interface OptimizeStatusResponse {
  optimizable: boolean;
  review_count: number;
  /** The day of study the parameters were last fitted, unless never here. */
  fitted_at: string | null;
  /** Reviews since then, or every review if never. */
  reviews_since: number;
  days_since: number | null;
  /** After `after_reviews` reviews or `after_days` days, whichever comes first. */
  suggested: boolean;
  after_reviews: number;
  after_days: number;
  /** Whether the host fits them by itself (`auto_optimize` on the root of the sync chain). */
  auto_optimize: boolean;
  optimizing: boolean;
  proposal: OptimizeProposal | null;
}

/** `spares_core::schema::review::RescheduleResponse` */
export interface RescheduleResponse {
  /** The event logging the cards it changed; undone by id only. */
  event_id: number | null;
}

export interface StatisticsGraphsResponse {
  mature_threshold_days: number;
  /** The local date of the request's `date`. */
  date: string;
  period_start: string;
  today: TodayStats;
  forecast: ForecastDay[];
  /** Only days with reviews or crams. */
  reviews: ReviewDay[];
  reviews_summary: ReviewsSummary;
  true_retention: RetentionRow[];
  answer_buttons: AnswerButtons;
  /** By local hour, 0 to 23. */
  hourly: HourStats[];
  card_counts: CardCounts;
  /** Only days with cards added. */
  added: { date: string; cards: number }[];
  distributions: Distributions;
}

export interface ReviewHistoryResponse {
  // Keyed by local `YYYY-MM-DD`, only dates with reviews
  review_count_by_date: Record<string, number>;
  // Likewise for crams, which the heatmap adds to the reviews
  cram_count_by_date: Record<string, number>;
  // The day under way as the counts tell days apart, `YYYY-MM-DD`: before the hour days start at
  // (`day_starts_at`, 4 AM by default), the date before the calendar's
  today: string;
}

export interface NoteResponse {
  id: number;
  data: string;
  parser_id: number;
  parser_name: string;
  keywords: string[];
  tags: string[];
  custom_data: Record<string, unknown>;
  // `null` if unpopulated
  linked_notes: LinkedNote[] | null;
  card_count: number;
  created_at: string;
  updated_at: string;
}

/** Settings of an image occlusion block in a note. The paths are absolute paths on the server. */
export interface ImageOcclusionData {
  original_image_filepath: string;
  clozes_filepath: string;
  front_conceal: string;
  back_reveal: string;
  back_emphasis: boolean;
}

/** What the image occlusion editor starts from: the template and the user's editor settings. */
export interface ImageOcclusionEditorConfig {
  template: string;
  fill_color: string;
  stroke_color: string;
  stroke_width: number;
  font_size: number;
  font_family: string;
  initial_tool: string;
}

/** See `NoteGraphRequest` in `spares_core::schema::note`. */
export interface NoteGraphRequest {
  query: string;
  /** The queue's `linked_note_max_score` if `null`. */
  max_score: number | null;
  /** Every link up if `null`. */
  ancestor_depth: number | null;
  /** Links down to the notes that build on those found: every one if `null`, none if 0. */
  descendant_depth: number | null;
  /** Queries a note is put in the first group of, to color it by. */
  groups: string[];
  max_nodes: number;
}

export interface NoteGraphResponse {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Links matching no note's keyword: notes yet to be written. */
  unmatched: UnmatchedLink[];
  truncated: boolean;
  links_generated: boolean;
}

export interface GraphNode {
  id: number;
  keywords: string[];
  tags: string[];
  /** `unmatched` is never sent: the graph page makes those nodes for unmatched links. */
  relation: GraphRelation;
  /** Links from the nearest note the query found. */
  depth: number;
  /** The index of the first group whose query finds it. */
  group: number | null;
  cards: GraphNodeCards;
}

/** Why a note is in a graph: the query found it, a note found builds on it, or it builds on one. */
export type GraphRelation =
  "found" | "prerequisite" | "dependent" | "unmatched";

export interface UnmatchedLink {
  /** The note with the link. */
  from: number;
  keyword: string;
}

/** Suspended cards are counted only as suspended. */
export interface GraphNodeCards {
  new: number;
  learning: number;
  review: number;
  suspended: number;
}

/**
 * `from` builds on `to`: it links to `to`, or `to` has a dependent link to it. For a `related`
 * link, where neither builds on the other, `from` is the note with the link.
 */
export interface GraphEdge {
  from: number;
  to: number;
  keyword: string;
  score: number | null;
  relation: LinkRelation;
}

export interface LinkedNote {
  searched_keyword: string;
  linked_note_id: number | null;
  matched_keyword: string | null;
  relation: LinkRelation;
}

// Externally-tagged serde enum
export type NotesSelector = { Ids: number[] } | { Query: string } | "All";

export interface CreateNoteRequest {
  data: string;
  keywords: string[];
  tags: string[];
  // Suspends all of its cards
  is_suspended: boolean;
  custom_data: Record<string, unknown>;
}

export interface MatchedKeywordResponse {
  matched_keyword: string;
  note_id: number;
  score: number;
}

export interface UnmatchedKeywordResponse {
  note_id: number;
  searched_keyword: string;
}

export interface NoteLink {
  parent_note_id: number;
  // `null` if no note matched the searched keyword
  linked_note_id: number | null;
  order: number;
  searched_keyword: string;
  matched_keyword: string | null;
  score: number | null;
}

export interface NoteRenderResponse {
  parser_name: string;
  rendered_path: string;
  // HTML shown instead of the rendered file, if the note has it (Markdown)
  web_path?: string;
  // Present iff the parser's sources are compiled in the browser (Typst)
  browser_source?: string;
}

/** One way of writing something in a note, from `/api/parsers/{id}/syntax`. */
export interface SyntaxExample {
  title: string;
  /** What it does. Text in backticks is code. */
  about: string;
  source: string;
  cards: CardPreview[];
}

/** A card of a `SyntaxExample`, as plain text: its front, with blanks or hints for what it hides. */
export interface CardPreview {
  front: string;
  answers: string[];
}

/** A field a search query can use, from `/api/search/fields`. */
export interface SearchField {
  name: string;
  kind: string;
  example: string;
  about: string;
  sortable: boolean;
  /** The values it can hold, if they are a few names, like `review` for `c.state`. */
  values: string[];
}

/** An example query, from `/api/search/examples`. */
export interface QueryExample {
  /** The group it is listed under. */
  section: string;
  query: string;
  /** What it finds. Text in backticks is code. */
  about: string;
}

/** A note a search found, and where its text has what the search looks for, as `[start, end)` string indices. */
export interface NoteMatch {
  note: NoteResponse;
  parser_name: string;
  matches: [number, number][];
  /** When any of its cards was last reviewed, leaving out crams, or `null` if none was */
  last_reviewed: string | null;
}

/** What a card's reviews say about it, counted as the search fields `c.interval`, `c.reps`, `c.lapses` and `c.reviewed` are. */
export interface CardStats {
  /** `null` for a card never reviewed */
  interval_days: number | null;
  reps: number;
  lapses: number;
  last_reviewed: string | null;
  /** How likely the card is to be recalled now, from 0 to 1, or `null` for a new card or one never reviewed */
  retrievability: number | null;
  /** Whether the card lapsed at least `lapses_threshold` times */
  leech: boolean;
}

/** A card a search found, with what its table shows besides the card. */
export interface CardMatch {
  card: CardResponse;
  parser_name: string;
  /** The start of its note's text */
  note_preview: string;
  /** Its note's tags, leaving out filtered tags */
  note_tags: string[];
  /** Its own tags, which its note doesn't have */
  card_tags: string[];
  stats: CardStats;
}

/** A field a search page is sorted by first, like a clicked column, as `sort_by_asc` takes it. */
export interface SearchSort {
  field: string;
  descending: boolean;
}

/** One page of a search's results, and how many there are on every page together. */
export interface SearchPage<T> {
  total: number;
  items: T[];
}

/** A search saved by name on the server, shared with `spares saved-search`. */
export interface SavedSearch {
  name: string;
  query: string;
}

/** `POST /api/notes/share/summary`: `spares_core::schema::note::ExportNotesResponse` of an export to share, with its media's names but not their contents. */
export interface ShareSummary {
  /** Each file of notes, by its parser's file extension */
  files: Record<string, string>;
  private_notes_left_out: number;
  media: { kind: "image" | "audio" | "image-occlusion"; name: string }[];
  /** Paths the notes name with no file there, which stay as they are */
  missing_media: string[];
}

/** `spares_core::api::note::import::ImportRequestOptions`: `spares import`'s options. */
export interface ImportRequestOptions {
  /** Without one, each file's parser is the one that finds notes in it */
  parser: string | null;
  to_parser: string | null;
  dry_run: boolean;
  strip_liveness: boolean;
}

/** `spares_core::api::note::import::ImportNoteSummary` */
export interface ImportNoteSummary {
  file: string;
  parser: string;
  action: "add" | "update" | "delete";
  note_id: number | null;
  card_count: number;
  tags: string[];
  keywords: string[];
  is_suspended: boolean;
  live: boolean;
  warnings: string[];
  data: string;
}

/** `spares_core::api::note::import::RewrittenFile` */
export interface RewrittenFile {
  file: number;
  name: string;
  contents: string;
}

/** `spares_core::api::note::import::ImportResponse` */
export interface ImportResponse {
  dry_run: boolean;
  notes: ImportNoteSummary[];
  skipped_note_count: number;
  deleted_live_note_ids: number[];
  rewritten_files: RewrittenFile[];
  /** The import's first event: undoing it with its group reverts the import */
  event_id: number | null;
  media_count: number;
  missing_media: string[];
  skipped_files: string[];
}

/** `spares_core::api::note::share_bundle::ImportSharedResponse` */
export interface ImportSharedResponse {
  note_count: number;
  event_ids: number[];
  parsers_added: string[];
  media_count: number;
  missing_media: string[];
  skipped_files: string[];
}

/** One week of a {@link GoalPartStatus}. */
export interface GoalWeek {
  /** The week's first day, `YYYY-MM-DD` */
  start: string;
  /** Cards first studied by the end of the week */
  learned: number;
  /** Reviews of the cards during the week, crams left out */
  reviews: number;
  /** Minutes spent on the cards during the week, crams included */
  minutes: number;
}

/**
 * How one of a goal's milestones (`milestones` of `[goals.<name>]`) is going, for the whole goal or
 * one of its parts.
 */
export interface GoalMilestoneStatus {
  /** The day it is due by the start of, `YYYY-MM-DD` */
  date: string;
  /** The most cards that should still be new then (a part's share of the goal's target) */
  new_left: number;
  /** Days from today to `date`, 0 once it has come */
  days_left: number;
  /** New cards a day that reach the target in time; `null` once the day has come */
  new_per_day_needed: number | null;
  /** Whether the last 14 days' pace reaches the target in time; `null` once the day has come */
  on_track: boolean | null;
  /** Once the day has come, the cards that were new when it started */
  new_on_day: number | null;
  /** Once the day has come, whether `new_on_day` was at most `new_left` */
  met: boolean | null;
  /** While the day is to come, the new cards the projection leaves then: fewest and most */
  projected_new_left: [number, number] | null;
}

/**
 * When a goal's (or part's) new cards would be learned with today's presets, limits, orders,
 * sibling burying and linked notes, studying every day. Random orders give a range.
 */
export interface GoalPartProjection {
  /** The day the last new card is first studied, in the earliest run; `null` if never */
  learned_earliest: string | null;
  /** Likewise, in the latest run */
  learned_latest: string | null;
  /** New cards still new when `learned_by` starts: fewest and most */
  new_left_at_learned_by: [number, number];
}

/** How much of a preset's new-card limit the goal's cards would leave unused, held back by links. */
export interface GoalPresetUse {
  name: string;
  new_limit: number;
  new: number;
  /** The share of the limit unused while cards waited on linked notes, before `learned_by` */
  lost_to_links: number;
}

/** How the cards of a goal, or of one of its parts, are going. */
export interface GoalPartStatus {
  name: string;
  /** The search of its cards, to review them with */
  query: string;
  cards: number;
  new: number;
  learning: number;
  review: number;
  suspended: number;
  /** Cards first studied in the last 14 days */
  learned_recently: number;
  /** New cards a day that learn the rest by `learned_by`; `null` once it has come */
  new_per_day_needed: number | null;
  /** When the new cards would all be learned at the last 14 days' pace */
  projected_learned_by: string | null;
  /** The average recall predicted on the goal's day, without another review */
  mean_recall_on_date: number | null;
  /** Studied cards below the goal's retention (0.9 if none) on the day without another review */
  below_retention_on_date: number;
  /** The last weeks, oldest first */
  weeks: GoalWeek[];
  /** The goal's milestones, in order, with this part's share of each target */
  milestones: GoalMilestoneStatus[];
  /** When its new cards would be learned with today's settings, unlike `projected_learned_by` */
  projection: GoalPartProjection | null;
}

/** A goal's search and its parts', to review them with (`GET /api/goals`). */
export interface GoalSearch {
  name: string;
  query: string;
  /** Each part's name and the search of its cards */
  parts: [string, string][];
}

/** A `[goals.<name>]` table of config.toml and how it is going (`spares goal status`). */
export interface GoalStatus {
  name: string;
  date: string;
  learned_by: string;
  retention: number | null;
  days_to_learned_by: number;
  days_to_date: number;
  /** The whole goal first, then each of its parts */
  parts: GoalPartStatus[];
  /** Presets with a new-card limit that the goal's new cards use */
  presets: GoalPresetUse[];
  /**
   * Minutes a day the next week would take at these limits, not counting the new cards' later
   * reviews; `null` with no reviews to go on
   */
  minutes_per_day: number | null;
}
