use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::{build_validator, Validator};
use crate::errors::{as_internal, val_line_error, ErrorKind, LocItem, ValError, ValLineError, ValResult};
use crate::standalone_validators::validate_dict;
use crate::utils::{dict, dict_get, py_error};
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct ModelField {
    name: String,
    // alias: Option<String>,
    required: bool,
    validator: Box<dyn Validator>,
}

#[derive(Debug, Clone)]
pub struct ModelValidator {
    fields: Vec<ModelField>,
    extra_behavior: ExtraBehavior,
    extra_validator: Option<Box<dyn Validator>>,
    // config: Option<Py<PyDict>>,
}

impl ModelValidator {
    pub const EXPECTED_TYPE: &'static str = "model";
}

impl Validator for ModelValidator {
    fn build(dict: &PyDict, _config: Option<&PyDict>) -> PyResult<Self> {
        let config = dict_get!(dict, "config", &PyDict);

        let extra_behavior = ExtraBehavior::from_config(config)?;
        let extra_validator = match extra_behavior {
            ExtraBehavior::Allow => match dict_get!(dict, "extra_validator", &PyDict) {
                Some(v) => Some(build_validator(v, config)?),
                None => None,
            },
            _ => None,
        };

        let fields_dict: &PyDict = match dict_get!(dict, "fields", &PyDict) {
            Some(fields) => fields,
            None => {
                // allow an empty model, is this is a good idea?
                return Ok(Self {
                    fields: vec![],
                    extra_behavior,
                    extra_validator,
                });
            }
        };
        let mut fields: Vec<ModelField> = Vec::with_capacity(fields_dict.len());

        for (key, value) in fields_dict.iter() {
            let field_dict: &PyDict = value.cast_as()?;

            fields.push(ModelField {
                name: key.to_string(),
                // alias: dict_get!(field_dict, "alias", String),
                required: dict_get!(field_dict, "required", bool).unwrap_or(false),
                validator: build_validator(field_dict, config)?,
            });
        }
        Ok(Self {
            fields,
            extra_behavior,
            extra_validator,
        })
    }

    fn validate(&self, py: Python, input: &PyAny, _data: &PyDict) -> ValResult<PyObject> {
        let dict: &PyDict = validate_dict(py, input)?;
        let output_dict = PyDict::new(py);
        let mut errors: Vec<ValLineError> = Vec::new();
        let mut fields_set: HashSet<String> = HashSet::with_capacity(dict.len());

        for field in &self.fields {
            if let Some(value) = dict.get_item(field.name.clone()) {
                match field.validator.validate(py, value, output_dict) {
                    Ok(value) => output_dict.set_item(field.name.clone(), value).map_err(as_internal)?,
                    Err(ValError::LineErrors(line_errors)) => {
                        let loc = vec![LocItem::S(field.name.clone())];
                        for err in line_errors {
                            errors.push(err.prefix_location(&loc));
                        }
                    }
                    Err(err) => return Err(err),
                }
                fields_set.insert(field.name.clone());
            } else if field.required {
                errors.push(val_line_error!(
                    py,
                    dict,
                    kind = ErrorKind::Missing,
                    location = vec![LocItem::S(field.name.clone())]
                ));
            }
        }

        let (check_extra, forbid) = match self.extra_behavior {
            ExtraBehavior::Ignore => (false, false),
            ExtraBehavior::Allow => (true, false),
            ExtraBehavior::Forbid => (true, true),
        };
        if check_extra {
            for (key, value) in dict.iter() {
                let key_str: String = match key.extract() {
                    Ok(key) => key,
                    Err(_) => {
                        errors.push(val_line_error!(
                            py,
                            dict,
                            kind = ErrorKind::InvalidKey,
                            location = vec![LocItem::from_py_repr(key)?]
                        ));
                        continue;
                    }
                };
                fields_set.insert(key_str.clone());
                let loc = vec![LocItem::S(key_str)];

                if forbid {
                    errors.push(val_line_error!(
                        py,
                        dict,
                        kind = ErrorKind::ExtraForbidden,
                        location = loc
                    ));
                } else if let Some(ref validator) = self.extra_validator {
                    match validator.validate(py, value, output_dict) {
                        Ok(value) => output_dict.set_item(key.clone(), value).map_err(as_internal)?,
                        Err(ValError::LineErrors(line_errors)) => {
                            for err in line_errors {
                                // TODO I don't think this clone is necessary, but the compiler disagrees
                                errors.push(err.prefix_location(&loc));
                            }
                        }
                        Err(err) => return Err(err),
                    }
                } else {
                    output_dict.set_item(key.clone(), value.clone()).map_err(as_internal)?;
                }
            }
        }

        if errors.is_empty() {
            Ok(dict!(py, "values" => output_dict, "fields_set" => fields_set))
        } else {
            Err(ValError::LineErrors(errors))
        }
    }

    fn clone_dyn(&self) -> Box<dyn Validator> {
        Box::new(self.clone())
    }
}

#[derive(Debug, Clone)]
enum ExtraBehavior {
    Allow,
    Ignore,
    Forbid,
}

impl ExtraBehavior {
    pub fn from_config(config: Option<&PyDict>) -> PyResult<Self> {
        match config {
            Some(dict) => {
                let b = dict_get!(dict, "behaviour", String);
                match b {
                    Some(s) => match s.as_str() {
                        "allow" => Ok(ExtraBehavior::Allow),
                        "ignore" => Ok(ExtraBehavior::Ignore),
                        "forbid" => Ok(ExtraBehavior::Forbid),
                        _ => py_error!(r#"Invalid extra_behavior: "{}""#, s),
                    },
                    None => Ok(ExtraBehavior::Ignore),
                }
            }
            None => Ok(ExtraBehavior::Ignore),
        }
    }
}
