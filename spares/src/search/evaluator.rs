use crate::{
    LibraryError,
    model::{Card, CardId, Note, NoteId},
    search::{Atom, Op, TokenTree, parser::Parser},
};
use miette::{Error, Report, miette};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

pub struct Evaluator<'de> {
    // whole: &'de str,
    parser: Parser<'de>,
}

impl<'de> Evaluator<'de> {
    pub fn new(input: &'de str) -> Self {
        Self {
            // whole: input,
            parser: Parser::new(input),
        }
    }

    fn evaluate(self, internal_output_type: EvaluatorReturnItemType) -> Result<String, Report> {
        let token_tree = self.parser.parse_expression()?;
        let mut context = EvaluationContext::new();
        context.root_context = true;
        token_tree.evaluate(&mut context)?;
        Ok(context.build_query(internal_output_type))
    }

    fn evaluate_with_parser(
        self,
        internal_output_type: EvaluatorReturnItemType,
    ) -> Result<String, Report> {
        let token_tree = self.parser.parse_expression()?;
        let mut context = EvaluationContext::new();
        context.root_context = true;
        context.table_requirements.needs_parser = true;
        token_tree.evaluate(&mut context)?;
        Ok(context.build_query(internal_output_type))
    }

    pub async fn get_notes(self, db: &SqlitePool) -> Result<Vec<(Note, String)>, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct EnrichedNote {
            #[sqlx(flatten)]
            note: Note,
            #[sqlx(rename = "name")]
            parser_name: String,
        }
        let query_str = self
            .evaluate_with_parser(EvaluatorReturnItemType::Notes)
            .map_err(|e| crate::Error::Library(LibraryError::Search(e.to_string())))?;
        dbg!(&query_str);
        let enriched_cards: Vec<EnrichedNote> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        let result = enriched_cards
            .into_iter()
            .map(|x| (x.note, x.parser_name))
            .collect::<Vec<_>>();
        Ok(result)
    }

    pub async fn get_note_ids(self, db: &SqlitePool) -> Result<Vec<NoteId>, crate::Error> {
        let query_str = self
            .evaluate(EvaluatorReturnItemType::NoteIds)
            .map_err(|e| crate::Error::Library(LibraryError::Search(e.to_string())))?;
        dbg!(&query_str);
        let note_ids_tups: Vec<(NoteId,)> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        Ok(note_ids_tups.into_iter().map(|(x,)| x).collect::<Vec<_>>())
    }

    pub async fn get_cards(self, db: &SqlitePool) -> Result<Vec<(Card, String)>, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct EnrichedCard {
            #[sqlx(flatten)]
            card: Card,
            #[sqlx(rename = "name")]
            parser_name: String,
        }
        let query_str = self
            .evaluate_with_parser(EvaluatorReturnItemType::Cards)
            .map_err(|e| crate::Error::Library(LibraryError::Search(e.to_string())))?;
        dbg!(&query_str);
        let enriched_cards: Vec<EnrichedCard> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        let result = enriched_cards
            .into_iter()
            .map(|x| (x.card, x.parser_name))
            .collect::<Vec<_>>();
        Ok(result)
    }

    pub async fn get_card_ids(self, db: &SqlitePool) -> Result<Vec<CardId>, crate::Error> {
        let query_str = self
            .evaluate(EvaluatorReturnItemType::CardIds)
            .map_err(|e| crate::Error::Library(LibraryError::Search(e.to_string())))?;
        dbg!(&query_str);
        let card_ids_tups: Vec<(CardId,)> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        Ok(card_ids_tups.into_iter().map(|(x,)| x).collect::<Vec<_>>())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Field {
    Note(NoteField),
    Card(CardField),
    SortAsc(String),
    SortDesc(String),
}

impl Default for Field {
    fn default() -> Self {
        Self::Note(NoteField::default())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
enum NoteField {
    Id,
    #[default]
    Data,
    CreatedAt,
    UpdatedAt,
    ParserName,
    Tag,
    Keyword,
    CustomData(String),
    LinkedTo,
}

#[derive(Debug, Clone, PartialEq)]
enum CardField {
    Id,
    CreatedAt,
    UpdatedAt,
    Due,
    Stability,
    Difficulty,
    DesiredRetention,
    // SpecialState,
    Suspended,
    UserBuried,
    SchedulerBuried,
    State,
    CustomData(String),
    Rated,
    Count,
}

#[derive(Debug, Clone, PartialEq)]
enum FieldType {
    Integer,
    Float,
    String,
    DateTime,
    Json,
    Boolean,
}

impl Field {
    fn from_str(input: &[&str]) -> Result<Field, Error> {
        if input.is_empty() || input[0].is_empty() {
            return Ok(Self::default());
        }
        let value_str = input[0];
        let normalized_value_str = if value_str.starts_with("card.") {
            value_str.replacen("card.", "c.", 1)
        } else {
            value_str.to_string()
        };
        if input.len() > 1
            && normalized_value_str.as_str() != "custom_data"
            && normalized_value_str.as_str() != "c.custom_data"
        {
            return Err(miette!(
                "Found an unexpected second field of `{}` when `{}` only accepts 1 field",
                input[1],
                input[0]
            ));
        }
        match normalized_value_str.as_str() {
            "id" => Ok(Field::Note(NoteField::Id)),
            "data" => Ok(Field::Note(NoteField::Data)),
            "created_at" => Ok(Field::Note(NoteField::CreatedAt)),
            "updated_at" => Ok(Field::Note(NoteField::UpdatedAt)),
            "parser_name" => Ok(Field::Note(NoteField::ParserName)),
            "tag" => Ok(Field::Note(NoteField::Tag)),
            "keyword" => Ok(Field::Note(NoteField::Keyword)),
            "custom_data" => {
                // if input.len() != 2 {
                //     dbg!(&input);
                //     return Err(miette!("Custom data expects 2 inputs"));
                // }
                Ok(Field::Note(NoteField::CustomData(
                    input.get(1).map(|x| (*x).to_string()).unwrap_or_default(),
                )))
            }
            "linked_to" => Ok(Field::Note(NoteField::LinkedTo)),
            "c.id" => Ok(Field::Card(CardField::Id)),
            "c.created_at" => Ok(Field::Card(CardField::CreatedAt)),
            "c.updated_at" => Ok(Field::Card(CardField::UpdatedAt)),
            "c.due" => Ok(Field::Card(CardField::Due)),
            "c.stability" => Ok(Field::Card(CardField::Stability)),
            "c.difficulty" => Ok(Field::Card(CardField::Difficulty)),
            "c.desired_retention" => Ok(Field::Card(CardField::DesiredRetention)),
            // "c.special_state" => Ok(Field::Card(CardField::SpecialState)),
            "c.suspended" => Ok(Field::Card(CardField::Suspended)),
            "c.user_buried" => Ok(Field::Card(CardField::UserBuried)),
            "c.scheduler_buried" => Ok(Field::Card(CardField::SchedulerBuried)),
            "c.state" => Ok(Field::Card(CardField::State)),
            "c.custom_data" => {
                // if input.len() != 2 {
                //     return Err(miette!("Custom data expects 2 inputs"));
                // }
                // Ok(Field::Card(CardField::CustomData(input[1].to_string())))
                Ok(Field::Card(CardField::CustomData(
                    input.get(1).map(|x| (*x).to_string()).unwrap_or_default(),
                )))
            }
            "c.rated" => Ok(Field::Card(CardField::Rated)),
            "c.count" => Ok(Field::Card(CardField::Count)),
            "sort_by_asc" => Ok(Field::SortAsc(String::new())),
            "sort_by_desc" => Ok(Field::SortDesc(String::new())),
            f => Err(miette!("Unrecognized field: {}", f)),
        }
    }

    fn to_sql_str(&self, value_str: &str) -> String {
        let with_op = move |field_name: &str| format!("{} {}", field_name, value_str);
        match self {
            Field::Note(note_field) => match note_field {
                NoteField::Id => with_op("n.id"),
                NoteField::Data => with_op("n.data"),
                NoteField::CreatedAt => with_op("n.created_at"),
                NoteField::UpdatedAt => with_op("n.updated_at"),
                NoteField::ParserName => with_op("p.name"),
                NoteField::Tag => {
                    // format!(
                    //     "n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name {})",
                    //     value_str
                    // )
                    format!(
                        "c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name {} UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name {})",
                        value_str, value_str
                    )
                }
                NoteField::Keyword => with_op("n.keywords"),
                NoteField::CustomData(json_path) => {
                    format!("json_extract(n.custom_data, '{}') {}", json_path, value_str)
                }
                NoteField::LinkedTo => {
                    format!(
                        "EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.linked_note_id {})",
                        value_str
                    )
                }
            },
            Field::Card(card_field) => match card_field {
                CardField::Id => with_op("c.id"),
                CardField::CreatedAt => with_op("c.created_at"),
                CardField::UpdatedAt => with_op("c.updated_at"),
                CardField::Due => with_op("c.due"),
                CardField::Stability => with_op("c.stability"),
                CardField::Difficulty => with_op("c.difficulty"),
                CardField::DesiredRetention => with_op("c.desired_retention"),
                // CardField::SpecialState => "c.special_state".to_string(),
                CardField::Suspended => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 1)".to_string()
                    } else {
                        "c.special_state = 1".to_string()
                    }
                }
                CardField::UserBuried => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 2)".to_string()
                    } else {
                        "c.special_state = 2".to_string()
                    }
                }
                CardField::SchedulerBuried => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 3)".to_string()
                    } else {
                        "c.special_state = 3".to_string()
                    }
                }
                CardField::State => with_op("c.state"),
                CardField::CustomData(json_path) => {
                    format!("json_extract(c.custom_data, '{}') {}", json_path, value_str)
                }
                CardField::Rated => {
                    format!(
                        "EXISTS (SELECT 1 FROM review_log rl WHERE rl.card_id = c.id AND rl.rating {})",
                        value_str
                    )
                }
                CardField::Count => {
                    with_op("(SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id)")
                }
            },
            Field::SortAsc(_) | Field::SortDesc(_) => String::new(),
        }
    }

    fn get_field_type(&self) -> FieldType {
        match self {
            Field::Note(note_field) => match note_field {
                NoteField::Id | NoteField::LinkedTo => FieldType::Integer,
                NoteField::Data | NoteField::Keyword | NoteField::ParserName | NoteField::Tag => {
                    FieldType::String
                }
                NoteField::CreatedAt | NoteField::UpdatedAt => FieldType::DateTime,
                NoteField::CustomData(_) => FieldType::Json,
            },
            Field::Card(card_field) => match card_field {
                CardField::Id | CardField::Rated | CardField::State | CardField::Count => {
                    FieldType::Integer
                }
                CardField::CreatedAt | CardField::UpdatedAt | CardField::Due => FieldType::DateTime,
                CardField::Stability | CardField::Difficulty | CardField::DesiredRetention => {
                    FieldType::Float
                }
                // CardField::SpecialState => {
                CardField::Suspended | CardField::UserBuried | CardField::SchedulerBuried => {
                    FieldType::Boolean
                }
                CardField::CustomData(_) => FieldType::Json,
            },
            Field::SortAsc(_) | Field::SortDesc(_) => FieldType::String,
        }
    }

    fn to_order_by_expr(field: &Field) -> Result<String, Error> {
        let field_type = field.get_field_type();
        match field_type {
            FieldType::Integer | FieldType::Float | FieldType::DateTime => {}
            _ => {
                return Err(miette!(
                    "Can only sort by numeric or DateTime fields. Got `{:?}` of type `{:?}`",
                    field,
                    field_type
                ));
            }
        }
        match field {
            Field::Note(note_field) => match note_field {
                NoteField::Id | NoteField::LinkedTo => Ok("n.id".to_string()),
                NoteField::CreatedAt => Ok("n.created_at".to_string()),
                NoteField::UpdatedAt => Ok("n.updated_at".to_string()),
                // Unsupported types
                NoteField::Data
                | NoteField::ParserName
                | NoteField::Tag
                | NoteField::Keyword
                | NoteField::CustomData(_) => Err(miette!("Cannot sort by field `{:?}`", field)),
            },
            Field::Card(card_field) => match card_field {
                CardField::Id => Ok("c.id".to_string()),
                CardField::CreatedAt => Ok("c.created_at".to_string()),
                CardField::UpdatedAt => Ok("c.updated_at".to_string()),
                CardField::Due => Ok("c.due".to_string()),
                CardField::Stability => Ok("c.stability".to_string()),
                CardField::Difficulty => Ok("c.difficulty".to_string()),
                CardField::DesiredRetention => Ok("c.desired_retention".to_string()),
                CardField::State => Ok("c.state".to_string()),
                CardField::Rated => Ok("c.rated".to_string()),
                CardField::Count => {
                    Ok("(SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id)".to_string())
                }
                // Unsupported types
                CardField::Suspended
                | CardField::UserBuried
                | CardField::SchedulerBuried
                | CardField::CustomData(_) => Err(miette!("Cannot sort by field `{:?}`", field)),
            },
            Field::SortAsc(_) | Field::SortDesc(_) => unreachable!(),
        }
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct TableRequirements {
    needs_parser: bool,
    // needs_tag: bool,
    needs_card: bool,
    needs_review_log: bool,
    needs_note_link: bool,
}

impl TableRequirements {
    fn analyze_field(field: Field) -> TableRequirements {
        let mut req = TableRequirements::default();
        match field {
            Field::Note(note_field) => match note_field {
                NoteField::ParserName => req.needs_parser = true,
                // NoteField::Tag => req.needs_tag = true,
                NoteField::Tag => req.needs_card = true,
                NoteField::LinkedTo => req.needs_note_link = true,
                _ => {}
            },
            Field::Card(_card_field) => {
                req.needs_card = true;
            }
            // Sort fields don't have requirements until we know the target field name,
            // which is determined during evaluation, not parsing
            Field::SortAsc(_) | Field::SortDesc(_) => {}
        }
        req
    }

    fn merge(&mut self, other: &TableRequirements) {
        self.needs_parser |= other.needs_parser;
        // self.needs_tag |= other.needs_tag;
        self.needs_card |= other.needs_card;
        self.needs_review_log |= other.needs_review_log;
        self.needs_note_link |= other.needs_note_link;
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EvaluatorReturnItemType {
    Notes,
    NoteIds,
    Cards,
    CardIds,
}

struct EvaluationContext<'a> {
    query_builder: QueryBuilder<'a, Sqlite>,
    params: Vec<String>,
    field: Option<Field>,
    value_type: Option<FieldType>,
    table_requirements: TableRequirements,
    where_clauses: Vec<String>,
    root_context: bool,
    order_by: Vec<String>,
}

impl EvaluationContext<'_> {
    fn new() -> Self {
        Self {
            query_builder: QueryBuilder::new(""),
            params: Vec::new(),
            field: None,
            value_type: None,
            table_requirements: TableRequirements::default(),
            where_clauses: Vec::new(),
            root_context: false,
            order_by: Vec::new(),
        }
    }

    fn build_query(&mut self, output_type: EvaluatorReturnItemType) -> String {
        // Start with base table
        match output_type {
            EvaluatorReturnItemType::Notes => {
                self.query_builder
                    .push("SELECT DISTINCT n.*, p.name FROM note n");
            }
            EvaluatorReturnItemType::NoteIds => {
                self.query_builder.push("SELECT DISTINCT n.id FROM note n");
            }
            EvaluatorReturnItemType::Cards => {
                self.query_builder
                    .push("SELECT DISTINCT c.*, p.name FROM card c");
                self.query_builder
                    .push(" LEFT JOIN note n ON n.id = c.note_id");
            }
            EvaluatorReturnItemType::CardIds => {
                self.query_builder.push("SELECT DISTINCT c.id FROM card c");
                self.query_builder
                    .push(" LEFT JOIN note n ON n.id = c.note_id");
            }
        }

        // Add necessary JOINs based on requirements
        if self.table_requirements.needs_card
            && !matches!(
                output_type,
                EvaluatorReturnItemType::Cards | EvaluatorReturnItemType::CardIds
            )
        {
            self.query_builder
                .push(" LEFT JOIN card c ON n.id = c.note_id");
        }
        if self.table_requirements.needs_parser {
            self.query_builder
                .push(" LEFT JOIN parser p ON n.parser_id = p.id");
        }
        // if self.table_requirements.needs_tag {
        //     self.query_builder
        //         .push(" LEFT JOIN note_tag nt ON n.id = nt.note_id")
        //         .push(" LEFT JOIN tag t ON nt.tag_id = t.id");
        // }
        if self.table_requirements.needs_note_link {
            self.query_builder
                .push(" LEFT JOIN note_link nl ON n.id = nl.parent_note_id");
        }

        // Add WHERE clause if we have conditions
        if !self.where_clauses.is_empty() {
            self.query_builder.push(" WHERE ");
            for (i, clause) in self.where_clauses.iter().enumerate() {
                if i > 0 {
                    self.query_builder.push(" AND ");
                }
                self.query_builder.push(clause);
            }
        }

        if !self.order_by.is_empty() {
            self.query_builder.push(" ORDER BY ");
            for (i, ob) in self.order_by.iter().enumerate() {
                if i > 0 {
                    self.query_builder.push(", ");
                }
                self.query_builder.push(ob);
            }
        }

        self.query_builder.sql().to_string()
    }

    fn add_where_clause(&mut self, clause: String) {
        self.where_clauses.push(clause);
    }
}

trait Evaluate {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error>;
}

impl Evaluate for TokenTree<'_> {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error> {
        let requirements = self.analyze_requirements()?;
        context.table_requirements.merge(&requirements);
        match self {
            TokenTree::Atom(atom) => atom.evaluate(context),
            TokenTree::Cons(op, trees) => match op {
                Op::And | Op::Or | Op::Group => {
                    let mut clauses = Vec::new();
                    if trees
                        .iter()
                        .any(|tree| matches!(tree, TokenTree::Atom(Atom::Nil)))
                    {
                        return Err(miette!("Found nil atom inside op."));
                    }
                    for tree in trees {
                        let mut inner_context = EvaluationContext::new();
                        tree.evaluate(&mut inner_context)?;
                        // Cannot use sort operations with 'or' operator
                        if matches!(op, Op::Or) && !inner_context.order_by.is_empty() {
                            return Err(miette!(
                                "Cannot use sort operations with 'or' operator. Sort operations cannot be used in alternative conditions."
                            ));
                        }
                        if let Some(clause) = inner_context.where_clauses.first() {
                            clauses.push(clause.clone());
                        }
                        // Merge order_by clauses
                        // This is necessary to not allow sort by conditions
                        // inside groups "c.stability>=2 or (a sort_by_asc=linked_to)"
                        context.order_by.extend(inner_context.order_by);
                        // Merge table requirements
                        context
                            .table_requirements
                            .merge(&inner_context.table_requirements);
                    }
                    let join_op = match op {
                        Op::And => " AND ",
                        Op::Or => " OR ",
                        Op::Group => " ",
                        _ => unreachable!("by outer match"),
                    };
                    if !clauses.is_empty() {
                        let clause_str = clauses.join(join_op);
                        // Only wrap in parentheses if there are multiple clauses
                        if clauses.len() > 1 {
                            context.add_where_clause(format!("({})", clause_str));
                        } else {
                            context.add_where_clause(clause_str);
                        }
                    }
                    Ok(())
                }
                Op::Minus => evaluate_minus(trees, context),
                Op::Colon => {
                    if context.root_context {
                        return Err(miette!("Missing operator"));
                    }
                    evaluate_colon(trees, context)
                }
                Op::GreaterThan
                | Op::GreaterThanEqual
                | Op::LessThan
                | Op::LessThanEqual
                | Op::Equal
                | Op::Tilde => evaluate_field_value(trees, context, *op),
            },
        }
    }
}

impl TokenTree<'_> {
    fn analyze_requirements(&self) -> Result<TableRequirements, Error> {
        match self {
            TokenTree::Atom(Atom::Field(field)) => {
                Ok(TableRequirements::analyze_field(Field::from_str(&[field])?))
            }
            TokenTree::Cons(_, trees) => {
                let mut requirements = TableRequirements::default();
                for tree in trees {
                    requirements.merge(&tree.analyze_requirements()?);
                }
                Ok(requirements)
            }
            TokenTree::Atom(_) => Ok(TableRequirements::default()),
        }
    }
}

fn evaluate_minus(trees: &[TokenTree], context: &mut EvaluationContext) -> Result<(), Error> {
    if trees.len() != 1 {
        return Err(miette!("Minus operation requires exactly one operand"));
    }
    let mut inner_context = EvaluationContext::new();
    if !matches!(
        trees[0],
        TokenTree::Cons(
            Op::GreaterThan
                | Op::GreaterThanEqual
                | Op::LessThan
                | Op::LessThanEqual
                | Op::Equal
                | Op::Tilde,
            _
        )
    ) {
        return Err(miette!("Minus operation requires a comparison operand."));
    }
    trees[0].evaluate(&mut inner_context)?;
    // Cannot negate sort operations
    if !inner_context.order_by.is_empty() {
        return Err(miette!(
            "Cannot negate sort operations. Sort operations cannot be used with the minus operator."
        ));
    }
    if let Some(clause) = inner_context.where_clauses.first() {
        context.add_where_clause(format!("NOT ({})", clause));
    }
    // Merge order_by clauses (should be empty due to check above, but merge anyway)
    // context.order_by.extend(inner_context.order_by);
    context
        .table_requirements
        .merge(&inner_context.table_requirements);
    Ok(())
}

fn evaluate_colon(trees: &[TokenTree], context: &mut EvaluationContext) -> Result<(), Error> {
    if trees.len() != 2 {
        return Err(miette!("Colon operation requires exactly two operands"));
    }
    let mut field_context = EvaluationContext::new();
    trees[0].evaluate(&mut field_context)?;
    if field_context.params.len() != 1 {
        return Err(miette!("Expected 1 field param in colon operator"));
    }

    let mut data_context = EvaluationContext::new();
    trees[1].evaluate(&mut data_context)?;
    if data_context.params.len() != 1 {
        return Err(miette!("Expected 1 data param in colon operator"));
    }

    context.field = Some(Field::from_str(&[
        &field_context.params[0],
        &data_context.params[0],
    ])?);

    context
        .table_requirements
        .merge(&data_context.table_requirements);
    Ok(())
}

fn evaluate_field_value(
    trees: &[TokenTree],
    context: &mut EvaluationContext,
    op: Op,
) -> Result<(), Error> {
    if trees.len() != 2 {
        return Err(miette!(
            "Field-value operation requires exactly two operands"
        ));
    }

    // First tree should be the field
    let mut field_context = EvaluationContext::new();
    if !matches!(
        trees[0],
        // `Op::Colon` is for Json
        TokenTree::Atom(Atom::Field(_)) | TokenTree::Cons(Op::Colon, _)
    ) {
        return Err(miette!("An operator requires a field to operate on."));
    }
    trees[0].evaluate(&mut field_context)?;
    let field = field_context
        .field
        .ok_or_else(|| miette!("Missing field"))?;
    let field_type = field.get_field_type();

    // Second tree should be the value
    let mut value_context = EvaluationContext::new();
    trees[1].evaluate(&mut value_context)?;
    let value_type = value_context
        .value_type
        .ok_or_else(|| miette!("Missing value"))?;

    // Special-case sorting keys
    if matches!(field, Field::SortAsc(_) | Field::SortDesc(_)) {
        if !matches!(op, Op::Equal) {
            return Err(miette!(
                "Sorting requires '=' operator, e.g., sort_by_asc=created_at"
            ));
        }
        let rhs_field_name = value_context
            .params
            .first()
            .ok_or_else(|| miette!("Missing sort field name"))?
            .to_string();
        let target_field = Field::from_str(&[&rhs_field_name])?;
        let order_expr = Field::to_order_by_expr(&target_field)?;
        // Merge table requirements for the sort target field
        context
            .table_requirements
            .merge(&TableRequirements::analyze_field(target_field.clone()));
        // Build order by expression
        match field {
            Field::SortAsc(_) => context.order_by.push(format!("{} ASC", order_expr)),
            Field::SortDesc(_) => context.order_by.push(format!("{} DESC", order_expr)),
            _ => {}
        }
        return Ok(());
    }
    match op {
        Op::Equal | Op::GreaterThan | Op::GreaterThanEqual | Op::LessThan | Op::LessThanEqual => {
            if let Some(mut value_param) = value_context.params.pop() {
                if matches!(op, Op::Equal) && matches!(field_type, FieldType::String) {
                    value_param = format!("\"{}\"", value_param);
                }
                value_context.params.push(format!("{} {}", op, value_param));
            }
        }
        Op::Tilde => {
            if let Some(value_param) = value_context.params.pop() {
                value_context
                    .params
                    .push(format!("LIKE '%{}%'", value_param));
            }
        }
        Op::Group | Op::And | Op::Or | Op::Minus | Op::Colon => unreachable!(),
    }
    let value_str = value_context.params.join(" ");

    // Ensure field and value have the same type
    match (&field_type, &value_type) {
        (FieldType::Integer, FieldType::Integer)
        | (FieldType::Float, FieldType::Float | FieldType::Integer)
        | (FieldType::String | FieldType::Json, _)
        | (FieldType::DateTime, FieldType::DateTime)
        | (FieldType::Boolean, FieldType::Boolean) => {}
        _ => {
            return Err(miette!(
                "The field `{:?}` has a type of `{:?}`. The provided value of `{}` has a type of `{:?}` which does not match the field's type.",
                field,
                field_type,
                value_str,
                value_type
            ));
        }
    }

    // Ensure field and operator have valid type
    // - Strings do not have an order
    // - Booleans do not have an order
    // - Booleans cannot use containment operator
    // - Numbers cannot use containment operator
    match (&field_type, &op) {
        (
            FieldType::String | FieldType::Boolean,
            Op::LessThan | Op::LessThanEqual | Op::GreaterThan | Op::GreaterThanEqual,
        )
        | (FieldType::Integer | FieldType::Float | FieldType::Boolean, Op::Tilde) => {
            return Err(miette!(
                "The field `{:?}` has a type of `{:?}` which cannot use operator `{}`",
                field,
                field_type,
                op,
            ));
        }
        _ => {}
    }

    let sql_condition = field.to_sql_str(&value_str);
    context.add_where_clause(sql_condition);
    Ok(())
}

impl Evaluate for Atom<'_> {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error> {
        match self {
            Atom::Field(field) => {
                context.field = Some(Field::from_str(&[field])?);
                context.params.push((*field).to_string());
                Ok(())
            }
            Atom::String(s) => {
                context.value_type = Some(FieldType::String);
                context.params.push(s.to_string());
                Ok(())
            }
            Atom::Integer(i) => {
                context.value_type = Some(FieldType::Integer);
                context.params.push(i.to_string());
                Ok(())
            }
            Atom::Float(f) => {
                context.value_type = Some(FieldType::Float);
                context.params.push(f.to_string());
                Ok(())
            }
            Atom::Boolean(b) => {
                context.value_type = Some(FieldType::Boolean);
                context.params.push(b.to_string());
                Ok(())
            }
            Atom::DateTime(d) => {
                context.value_type = Some(FieldType::DateTime);
                context.params.push(d.timestamp().to_string());
                Ok(())
            }
            Atom::Nil => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_queries() {
        let inputs = vec![
            (
                "c.stability>=2.1",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability >= 2.1",
            ),
            (
                "c.stability=2.1",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability = 2.1",
            ),
            // Only data search - no joins needed
            (
                "dog",
                "SELECT DISTINCT n.id FROM note n WHERE n.data LIKE '%dog%'",
            ),
            // Card count
            (
                "c.count=2",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) = 2",
            ),
            (
                "dog c.count>=2",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (n.data LIKE '%dog%' AND (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) >= 2)",
            ),
            // Tag search - needs tag joins
            (
                "tag=math",
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN note_tag nt ON n.id = nt.note_id LEFT JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\""),
                // "SELECT DISTINCT n.id FROM note n WHERE n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")",
            ),
            // Exclude tag
            (
                "-tag=math",
                // "SELECT DISTINCT n.id FROM note n WHERE NOT (n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\"))",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\"))",
            ),
            // Exclude linked to
            (
                "-linked_to=12",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE NOT (EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.linked_note_id = 12))",
            ),
            // Convert types
            (
                "tag=2 or tag=true",
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN note_tag nt ON n.id = nt.note_id LEFT JOIN tag t ON nt.tag_id = t.id WHERE (t.name = \"2\" OR t.name = \"true\")"),
                // "SELECT DISTINCT n.id FROM note n WHERE (n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"2\") OR n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"true\"))",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"2\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"2\") OR c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"true\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"true\"))",
            ),
            // Card search - needs card join
            (
                "-card.scheduler_buried=true",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (c.special_state = 3)",
            ),
            // Complex query - needs multiple joins
            (
                "dog and tag=math and -card.suspended=true",
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_tag nt ON n.id = nt.note_id LEFT JOIN tag t ON nt.tag_id = t.id WHERE ((n.data LIKE '%dog%' AND t.name = \"math\") AND NOT (c.special_state = 1))"),
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data LIKE '%dog%' AND n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")) AND NOT (c.special_state = 1))",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data LIKE '%dog%' AND c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")) AND NOT (c.special_state = 1))",
            ),
            (
                "dog and tag=math and card.suspended=false",
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_tag nt ON n.id = nt.note_id LEFT JOIN tag t ON nt.tag_id = t.id WHERE ((n.data LIKE '%dog%' AND t.name = \"math\") AND NOT (c.special_state = 1))"),
                // "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data LIKE '%dog%' AND n.id IN (SELECT note_id FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")) AND (c.special_state IS NULL OR c.special_state != 1))",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data LIKE '%dog%' AND c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\")) AND (c.special_state IS NULL OR c.special_state != 1))",
            ),
            // Custom data - no joins needed
            (
                "custom_data:\"$.x.y[1]\">=123",
                "SELECT DISTINCT n.id FROM note n WHERE json_extract(n.custom_data, '$.x.y[1]') >= 123",
            ),
        ];

        for (input, expected_sql) in inputs {
            dbg!(&input);
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(query_str, expected_sql);
        }
    }

    #[test]
    fn test_search_query_errors() {
        let inputs = [
            // Missing value
            "tag=",
            // Invalid value
            "tag:personal",
            "-tag:personal",
            // Invalid field
            "tags=math",
            // Field type and value type do not match
            "card.scheduler_buried=test",
            // Field type and operator type do not match
            "data>2",        // Strings do not have an order
            "id~2",          // Numbers cannot use containment operator
            "c.suspended~2", // Booleans cannot use containment operator
            "c.suspended>2", // Booleans do not have an order
            // String as field is not allowed
            "\"test\"=2",
            // Custom data with field, but no comparison op
            "custom_data:\"$.x.y[1]\"",
            "-custom_data:\"$.x.y[1]\"",
            "custom_data:\"$.x.y[1]\" and",
            // Dangling operator
            "tag=math and",
            "or tag=math",
        ];
        for input in inputs {
            dbg!(&input);
            let evaluator = Evaluator::new(input);
            let query_str_res = evaluator.evaluate(EvaluatorReturnItemType::NoteIds);
            dbg!(&query_str_res);
            assert!(query_str_res.is_err());
        }
    }

    #[test]
    fn test_sorting() {
        let inputs = vec![
            // Basic ascending sort on note field
            (
                "sort_by_asc=created_at",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.created_at ASC",
            ),
            // Basic descending sort on note field
            (
                "sort_by_desc=updated_at",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.updated_at DESC",
            ),
            // Sort by note id
            (
                "sort_by_asc=id",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.id ASC",
            ),
            // Sort by card field
            (
                "sort_by_asc=c.stability",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.stability ASC",
            ),
            // Sort by card due date
            (
                "sort_by_desc=c.due",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.due DESC",
            ),
            // Sort by numeric card fields
            (
                "sort_by_asc=c.difficulty",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.difficulty ASC",
            ),
            (
                "sort_by_desc=c.desired_retention",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.desired_retention DESC",
            ),
            // Sort by card id
            (
                "sort_by_asc=c.id",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.id ASC",
            ),
            // Sort by card created_at
            (
                "sort_by_desc=c.created_at",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.created_at DESC",
            ),
            // Sort by card count
            (
                "sort_by_asc=c.count",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) ASC",
            ),
            // Multiple sorts
            (
                "sort_by_desc=c.stability sort_by_asc=c.id",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.stability DESC, c.id ASC",
            ),
            // Sort combined with WHERE clause
            (
                "tag=math sort_by_asc=created_at",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\") ORDER BY n.created_at ASC",
            ),
            (
                "c.stability>=2 sort_by_desc=c.due",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability >= 2 ORDER BY c.due DESC",
            ),
            // Sort with linked_to field
            (
                "sort_by_asc=linked_to",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id ORDER BY n.id ASC",
            ),
            // Sort with parenthesis
            (
                "(tag=math and c.stability>=2) sort_by_asc=linked_to)",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE (c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\") AND c.stability >= 2) ORDER BY n.id ASC",
            ),
            // The following should be equivalent to the one above.
            (
                "tag=math and (c.stability>=2 sort_by_asc=linked_to)",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE (c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.name = \"math\" UNION SELECT c.id FROM card c JOIN note n ON c.note_id = n.id JOIN note_tag nt ON n.id = nt.note_id JOIN tag t ON nt.tag_id = t.id WHERE t.name = \"math\") AND c.stability >= 2) ORDER BY n.id ASC",
            ),
        ];

        for (input, expected_sql) in inputs {
            dbg!(&input);
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(query_str, expected_sql, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_sorting_errors() {
        let inputs = [
            // Cannot sort by string fields
            "sort_by_asc=tag",
            "sort_by_desc=data",
            "sort_by_asc=parser_name",
            "sort_by_desc=keyword",
            // Cannot sort by boolean fields
            "sort_by_asc=c.suspended",
            "sort_by_desc=c.user_buried",
            // Cannot sort by json fields
            "sort_by_desc=custom_data",
            // Cannot use non-equal operator for sorting
            "sort_by_asc>=created_at",
            "sort_by_desc~c.stability",
            // Invalid field name
            "sort_by_asc=invalid_field",
            "sort_by_asc=sort_by_asc",
            "sort_by_asc=sort_by_desc",
            // Cannot use certain operators for sorting
            "-sort_by_asc=linked_to",
            "c.stability>=2 or sort_by_asc=linked_to",
            // `and` is fine since that is implictly used everywhere
            // Cannot use sort by inside grouping
            "c.stability>=2 or (a sort_by_asc=linked_to)",
            // NOTE: Even though this is equivalent to `(c.stability >=2 or a) sort_by_asc=linked_to`, this creates more confusion, so it is not allowed at the moment.
            "c.stability>=2 -(a sort_by_asc=linked_to)",
        ];
        for input in inputs {
            dbg!(&input);
            let evaluator = Evaluator::new(input);
            let query_str_res = evaluator.evaluate(EvaluatorReturnItemType::NoteIds);
            dbg!(&query_str_res);
            assert!(
                query_str_res.is_err(),
                "Expected error for input: {}",
                input
            );
        }
    }
}
