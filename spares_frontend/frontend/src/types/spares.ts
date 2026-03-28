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
  cards_left_by_state: Record<string, number>;
  time_estimate: number; // seconds (DurationSeconds<i64>)
  linked_notes: ReviewLinkedNote[];
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

export interface SubmitStudyActionRequest {
  scheduler_name: string;
  action: { Rate: RatingSubmission } | { Bury: { card_id: number } };
}

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
  card_count: number;
  created_at: string;
  updated_at: string;
}
