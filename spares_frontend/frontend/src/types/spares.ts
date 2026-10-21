export interface Credentials {
  serverUrl: string;
  apiKey: string;
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
  card_back_web_path?: { CardBack: string } | { Note: string };
  card_front_raw_path: string;
  card_back_raw_path: { CardBack: string } | { Note: string };
  note_raw_path: string;
  parser_name: string;
  keywords: string[];
  // Present iff the card is reviewed by running an external command (CLI only)
  cli?: CliReviewInfo;
  // Present iff the parser's sources are compiled in the browser (Typst)
  browser_sources?: BrowserRenderSources;
  // Present iff the card asks for its answers to be typed (the cloze key `t`)
  typed_answer?: TypedAnswerInfo;
  // Present iff the card has audio clips
  audio?: CardAudio;
  cards_left_by_state: Record<string, number>;
  time_estimate: number; // seconds (DurationSeconds<i64>)
  linked_notes: ReviewLinkedNote[];
}

export interface CliReviewInfo {
  exec: string;
  surrounding: string;
}

// Internally-tagged serde enum
export type TypeAnswer =
  | { kind: 'diff'; ignore_case: boolean; ignore_combining: boolean }
  | { kind: 'manual' };

export interface TypedBlank {
  expected: string;
  hint?: string;
}

export interface TypedAnswerInfo {
  mode: TypeAnswer;
  // One per answer cloze, in reading order
  blanks: TypedBlank[];
}

// Absolute paths of audio clips, fetched from the render assets
export interface CardAudio {
  // Played when the card is shown
  front: string[];
  // Played when it is flipped. Clips on the front are not repeated.
  back: string[];
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
}

export interface Rating {
  id: number;
  description: string;
}

export interface RatingSubmission {
  card_id: number;
  rating: number;
  recall_duration: number; // seconds
  rate_duration: number;   // seconds
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
}

export interface SubmitStudyActionResponse {
  event_id: number | null;
}

// Externally-tagged serde enum
export type ReviewFilter = { Query: string } | { FilteredTag: { tag_id: number } };

export interface ReviewQueueRequest {
  filter: ReviewFilter | null;
  limit: number;
  // Also include cards due on the next `days_ahead` days
  days_ahead: number;
}

export interface ReviewQueueCard extends GetReviewCardResponse {
  due: string; // ISO 8601
  // The earliest the card may be shown, for a learning card held back by the learn ahead limit
  // (ISO 8601). Missing from queues downloaded before it was added.
  shown_from?: string | null;
}

export interface ReviewQueueResponse {
  // In the order a review would show them
  cards: ReviewQueueCard[];
  new_cards_left_today: number;
}

// The study actions that can be performed offline
export type OfflineStudyActionKind = { Rate: RatingSubmission } | { Bury: { card_id: number } };

export interface OfflineStudyAction {
  // Unique per action, so a resent action is not applied twice
  client_id: string;
  performed_at: string; // ISO 8601
  action: OfflineStudyActionKind;
}

export interface SyncOfflineActionsRequest {
  scheduler_name: string;
  actions: OfflineStudyAction[];
}

export type OfflineActionOutcome =
  | { Applied: { event_id: number | null } }
  | 'Duplicate'
  | { Rejected: { message: string } };

export interface SyncOfflineActionsResponse {
  // One per action, in the order they were submitted
  results: { client_id: string; outcome: OfflineActionOutcome }[];
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

export type SpecialState = 'Suspended' | 'UserBuried' | 'SchedulerBuried' | 'BuriedUntilLaterToday';
export type SpecialStateUpdate = 'Suspended' | 'Buried' | 'BuriedUntilLaterToday';

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
}

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
  /** `Cram` is a rating in a filtered tag that does not reschedule, which left the card's schedule alone. */
  kind: 'Review' | 'Forget' | 'Cram';
  reviewed_at: string;
  /** Relative to `scheduler_name`. */
  rating: number | null;
  scheduler_name: string;
  previous_state: number;
  scheduled_time: number | null; // seconds
  recall_duration: number | null; // seconds
  rate_duration: number | null;   // seconds
  tag_id: number | null;
  /** Replayed with the scheduler's current parameters. `null` after a forget. A cram keeps the one before it. */
  stability: number | null;
  difficulty: number | null;
  /** Whether this review was a lapse: failed, by its scheduler's ratings, after the card was learned. */
  lapse: boolean;
}

/** An event that undo or redo reversed. */
export interface EventSummary {
  id: number;
  /** The action the user originally took, e.g. "Rate card 3 (Good)", whether undone or redone. */
  description: string;
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

export const STATE_LABELS: Record<number, string> = {
  0: 'New',
  1: 'Learning',
  2: 'Review',
  3: 'Relearning',
};

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

export interface ForecastDay { date: string; learning: number; review: number }

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

export interface PassFail { passed: number; failed: number }

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

export interface HourStats { reviews: number; passed: number; crams: number; crams_passed: number }

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
export interface Histogram { edges: number[]; counts: number[]; mean: number | null }

export interface Distributions {
  interval: Histogram;
  stability: Histogram;
  difficulty: Histogram;
  /** `null` if the scheduler has no forgetting curve. */
  retrievability: Histogram | null;
  total_knowledge: number | null;
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
  max_nodes: number;
}

export interface NoteGraphResponse {
  nodes: GraphNode[];
  edges: GraphEdge[];
  truncated: boolean;
  links_generated: boolean;
}

export interface GraphNode {
  id: number;
  keywords: string[];
  tags: string[];
  /** Whether the query found it, rather than a note it found linking to it. */
  in_query: boolean;
  /** Links up from the nearest note the query found. */
  depth: number;
  cards: GraphNodeCards;
}

/** Suspended cards are counted only as suspended. */
export interface GraphNodeCards {
  new: number;
  learning: number;
  review: number;
  suspended: number;
}

/** `from` links to `to`, the note it builds on. */
export interface GraphEdge {
  from: number;
  to: number;
  keyword: string;
  score: number | null;
}

export interface LinkedNote {
  searched_keyword: string;
  linked_note_id: number | null;
  matched_keyword: string | null;
}

// Externally-tagged serde enum
export type NotesSelector = { Ids: number[] } | { Query: string } | 'All';

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
