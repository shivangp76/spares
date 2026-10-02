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
  card_front_raw_path: string;
  card_back_raw_path: { CardBack: string } | { Note: string };
  note_raw_path: string;
  parser_name: string;
  keywords: string[];
  // Present iff the card is reviewed by running an external command (CLI only)
  cli?: CliReviewInfo;
  // Present iff the parser's sources are compiled in the browser (Typst)
  browser_sources?: BrowserRenderSources;
  cards_left_by_state: Record<string, number>;
  time_estimate: number; // seconds (DurationSeconds<i64>)
  linked_notes: ReviewLinkedNote[];
}

export interface CliReviewInfo {
  exec: string;
  surrounding: string;
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
  created_at: number; // unix seconds
  updated_at: number; // unix seconds
}

export type TagSelector = { Id: number } | { Name: string };

export interface CreateTagRequest {
  name: string;
  description: string;
  query: string | null;
  auto_delete: boolean;
}

export interface UpdateTagRequest {
  tag_to_modify: TagSelector;
  name?: string;
  description?: string;
  // Omit to leave unchanged, `null` to clear
  query?: string | null;
  auto_delete?: boolean;
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

export interface UndoEventResponse {
  undone_event_ids: number[];
}

export const STATE_LABELS: Record<number, string> = {
  0: 'New',
  1: 'Learning',
  2: 'Review',
  3: 'Relearning',
};

export interface StatisticsResponse {
  cards_studied_count: number;
  recall_duration: number;
  rate_duration: number;
  card_count_by_state: Record<string, number>;
  due_count_by_state: Record<string, number>;
  due_count_by_date: Record<string, number>;
  advance_safe_count: number;
  postpone_safe_count: number;
}

export interface NoteResponse {
  id: number;
  data: string;
  parser_id: number;
  keywords: string[];
  tags: string[];
  custom_data: Record<string, unknown>;
  // `null` if unpopulated
  linked_notes: LinkedNote[] | null;
  card_count: number;
  created_at: string;
  updated_at: string;
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
  // Present iff the parser's sources are compiled in the browser (Typst)
  browser_source?: string;
}
