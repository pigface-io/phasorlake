//! The schema updates Iceberg applies in metadata alone, and the ones it
//! refuses.

use utilake_core::{Category, Diagnose, Diagnostic};

use crate::schema::{Field, Schema};
use crate::types::IcebergType;

/// One change to a table's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// A new optional column, with the next field id.
    AddColumn {
        /// Its name.
        name: String,
        /// Its type.
        ty: IcebergType,
        /// What it holds.
        doc: Option<String>,
    },
    /// A column dropped; its id is never given out again.
    DeleteColumn {
        /// The field.
        id: u32,
    },
    /// A column renamed; its data and id stay.
    RenameColumn {
        /// The field.
        id: u32,
        /// The new name.
        name: String,
    },
    /// A column's type changed.
    Promote {
        /// The field.
        id: u32,
        /// The new type.
        to: IcebergType,
    },
    /// A column that may now be null.
    MakeOptional {
        /// The field.
        id: u32,
    },
    /// A column that may no longer be null: never legal.
    MakeRequired {
        /// The field.
        id: u32,
    },
    /// The key.
    SetIdentifierFields {
        /// The fields, in key order.
        ids: Vec<u32>,
    },
}

/// Why Iceberg refuses an update.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    /// The update names a field the schema lacks.
    #[error("The schema has no field with id {id}.")]
    UnknownField {
        /// The id.
        id: u32,
    },
    /// Two columns would share a name.
    #[error(
        "The schema has a column named {name} already; Iceberg needs names unique within a table."
    )]
    DuplicateName {
        /// The name.
        name: String,
    },
    /// A type change that is not a promotion.
    #[error(
        "Column {name} cannot change from {from} to {to}: Iceberg promotes only a decimal to a larger precision of the same scale, or a date to a time. Values of the new type need a new column."
    )]
    NotAPromotion {
        /// The column.
        name: String,
        /// Its type.
        from: IcebergType,
        /// The type asked for.
        to: IcebergType,
    },
    /// An optional column made required.
    #[error(
        "Column {name} cannot become required: rows already written may hold nulls, and Iceberg never narrows a column."
    )]
    RequiredForbidden {
        /// The column.
        name: String,
    },
    /// A key field that Iceberg does not accept, or a change to one.
    #[error("Column {name} {reason}.")]
    Identifier {
        /// The column.
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
}

impl Diagnose for UpdateError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        let (code, column) = match self {
            Self::UnknownField { .. } => ("schema.iceberg_unknown_field", None),
            Self::DuplicateName { name } => ("schema.iceberg_duplicate_name", Some(name)),
            Self::NotAPromotion { name, .. } => ("schema.iceberg_not_a_promotion", Some(name)),
            Self::RequiredForbidden { name } => ("schema.iceberg_required_forbidden", Some(name)),
            Self::Identifier { name, .. } => ("schema.iceberg_identifier", Some(name)),
        };
        let d = Diagnostic::error(code, Category::Schema, msg);
        match column {
            Some(c) => d.with_column(c.clone()),
            None => d,
        }
    }
}

impl Schema {
    /// The schema after `update`, with the next schema id, or why Iceberg
    /// refuses it.
    pub fn apply(&self, update: &Update) -> Result<Self, UpdateError> {
        let mut next = self.clone();
        next.schema_id += 1;
        match update {
            Update::AddColumn { name, ty, doc } => {
                self.unique(name)?;
                next.last_column_id += 1;
                next.fields.push(Field {
                    id: next.last_column_id,
                    name: name.clone(),
                    ty: *ty,
                    required: false,
                    doc: doc.clone(),
                });
            }
            Update::DeleteColumn { id } => {
                let f = self.known(*id)?;
                self.not_key(
                    f,
                    "is part of the key and cannot be deleted; change the key first",
                )?;
                next.fields.retain(|g| g.id != *id);
            }
            Update::RenameColumn { id, name } => {
                self.known(*id)?;
                self.unique(name)?;
                next.field_mut(*id).name.clone_from(name);
            }
            Update::Promote { id, to } => {
                let f = self.known(*id)?;
                if !f.ty.promotes_to(*to) {
                    return Err(UpdateError::NotAPromotion {
                        name: f.name.clone(),
                        from: f.ty,
                        to: *to,
                    });
                }
                next.field_mut(*id).ty = *to;
            }
            Update::MakeOptional { id } => {
                let f = self.known(*id)?;
                self.not_key(f, "is part of the key, which Iceberg needs required")?;
                next.field_mut(*id).required = false;
            }
            Update::MakeRequired { id } => {
                let f = self.known(*id)?;
                if !f.required {
                    return Err(UpdateError::RequiredForbidden {
                        name: f.name.clone(),
                    });
                }
            }
            Update::SetIdentifierFields { ids } => {
                for id in ids {
                    let f = self.known(*id)?;
                    if !f.required {
                        return Err(UpdateError::Identifier {
                            name: f.name.clone(),
                            reason: "may be null, so it cannot be part of the key",
                        });
                    }
                    if !f.ty.can_identify() {
                        return Err(UpdateError::Identifier {
                            name: f.name.clone(),
                            reason: "is a float, which Iceberg does not accept in a key",
                        });
                    }
                }
                next.identifier_field_ids.clone_from(ids);
            }
        }
        Ok(next)
    }

    /// The schema after every update in turn.
    pub fn apply_all<'a>(
        &self,
        updates: impl IntoIterator<Item = &'a Update>,
    ) -> Result<Self, UpdateError> {
        updates
            .into_iter()
            .try_fold(self.clone(), |s, u| s.apply(u))
    }

    fn known(&self, id: u32) -> Result<&Field, UpdateError> {
        self.field(id).ok_or(UpdateError::UnknownField { id })
    }

    fn unique(&self, name: &str) -> Result<(), UpdateError> {
        match self.by_name(name) {
            Some(_) => Err(UpdateError::DuplicateName { name: name.into() }),
            None => Ok(()),
        }
    }

    fn not_key(&self, f: &Field, reason: &'static str) -> Result<(), UpdateError> {
        if self.identifier_field_ids.contains(&f.id) {
            return Err(UpdateError::Identifier {
                name: f.name.clone(),
                reason,
            });
        }
        Ok(())
    }

    fn field_mut(&mut self, id: u32) -> &mut Field {
        self.fields
            .iter_mut()
            .find(|f| f.id == id)
            .expect("the field was checked to exist")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(name: &str, ty: IcebergType) -> Update {
        Update::AddColumn {
            name: name.into(),
            ty,
            doc: None,
        }
    }

    /// A key column made required by building the field directly: Iceberg
    /// adds a required column only with a table, never by update.
    fn keyed() -> Schema {
        let mut s = Schema::default()
            .apply_all(&[
                add("T", IcebergType::Timestamptz),
                add("V", IcebergType::Double),
            ])
            .unwrap();
        s.fields[0].required = true;
        s.apply(&Update::SetIdentifierFields { ids: vec![1] })
            .unwrap()
    }

    fn code(r: Result<Schema, UpdateError>) -> String {
        r.unwrap_err().diagnostic().code.into_owned()
    }

    #[test]
    fn adds_rename_promote_and_delete_never_reuse_an_id() {
        let s = keyed();
        assert_eq!((s.schema_id, s.last_column_id), (3, 2));
        let s = s
            .apply_all(&[
                add("D", IcebergType::decimal(10, 2).unwrap()),
                Update::RenameColumn {
                    id: 2,
                    name: "V2".into(),
                },
                Update::Promote {
                    id: 3,
                    to: IcebergType::decimal(20, 2).unwrap(),
                },
                Update::MakeOptional { id: 2 },
                Update::DeleteColumn { id: 2 },
                add("E", IcebergType::String),
            ])
            .unwrap();
        let ids: Vec<_> = s.fields.iter().map(|f| (f.id, f.name.as_str())).collect();
        assert_eq!(ids, [(1, "T"), (3, "D"), (4, "E")]);
        assert_eq!(s.field(3).unwrap().ty, IcebergType::decimal(20, 2).unwrap());
        assert!(!s.field(4).unwrap().required);
        // A required column asked to stay required is no change.
        assert!(s.apply(&Update::MakeRequired { id: 1 }).is_ok());
    }

    #[test]
    fn refuses_what_iceberg_refuses() {
        let s = keyed();
        assert_eq!(
            code(s.apply(&Update::DeleteColumn { id: 9 })),
            "schema.iceberg_unknown_field"
        );
        assert_eq!(
            code(s.apply(&add("V", IcebergType::Long))),
            "schema.iceberg_duplicate_name"
        );
        assert_eq!(
            code(s.apply(&Update::RenameColumn {
                id: 2,
                name: "T".into()
            })),
            "schema.iceberg_duplicate_name"
        );
        assert_eq!(
            code(s.apply(&Update::Promote {
                id: 2,
                to: IcebergType::String
            })),
            "schema.iceberg_not_a_promotion"
        );
        assert_eq!(
            code(s.apply(&Update::MakeRequired { id: 2 })),
            "schema.iceberg_required_forbidden"
        );
        assert_eq!(
            code(s.apply(&Update::DeleteColumn { id: 1 })),
            "schema.iceberg_identifier"
        );
        assert_eq!(
            code(s.apply(&Update::MakeOptional { id: 1 })),
            "schema.iceberg_identifier"
        );
        assert_eq!(
            code(s.apply(&Update::SetIdentifierFields { ids: vec![2] })),
            "schema.iceberg_identifier"
        );
        let mut floaty = s.clone();
        floaty.fields[1].required = true;
        let e = floaty
            .apply(&Update::SetIdentifierFields { ids: vec![2] })
            .unwrap_err();
        assert!(e.to_string().contains("float"), "{e}");
        assert_eq!(e.diagnostic().column.as_deref(), Some("V"));
        assert_eq!(
            s.apply(&Update::DeleteColumn { id: 9 })
                .unwrap_err()
                .diagnostic()
                .column,
            None
        );
    }
}
