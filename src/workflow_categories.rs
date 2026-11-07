//! Workflow category configuration and pure picker tree construction.
use crate::error::{CruiseError, Result};
use indexmap::IndexMap;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
    ser::SerializeMap,
};
use std::collections::HashMap;
use std::fmt;

pub const PACKAGES_CATEGORY_NAME: &str = "Packages";
pub const OTHERS_CATEGORY_NAME: &str = "Others";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorkflowCategoryNode {
    pub workflows: Vec<WorkflowEntry>,
    pub children: IndexMap<String, WorkflowCategoryNode>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowEntry {
    pub name: String,
    pub description: Option<String>,
}
impl Serialize for WorkflowEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Some(desc) = &self.description {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry(&self.name, desc)?;
            map.end()
        } else {
            serializer.serialize_str(&self.name)
        }
    }
}
impl<'de> Deserialize<'de> for WorkflowEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Entry {
            Name(String),
            Described(IndexMap<String, String>),
        }
        match Entry::deserialize(deserializer)? {
            Entry::Name(name) => Ok(Self {
                name,
                description: None,
            }),
            Entry::Described(map) => {
                let mut entries = map.into_iter();
                match (entries.next(), entries.next()) {
                    (Some((name, description)), None) => Ok(Self {
                        name,
                        description: Some(description),
                    }),
                    _ => Err(serde::de::Error::custom(
                        "workflow entry must have exactly one description",
                    )),
                }
            }
        }
    }
}
impl Serialize for WorkflowCategoryNode {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(
            self.children.len() + usize::from(!self.workflows.is_empty()),
        ))?;
        if !self.workflows.is_empty() {
            map.serialize_entry("workflows", &self.workflows)?;
        }
        for (name, child) in &self.children {
            map.serialize_entry(name, child)?;
        }
        map.end()
    }
}
impl<'de> Deserialize<'de> for WorkflowCategoryNode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct NodeVisitor;
        impl<'de> Visitor<'de> for NodeVisitor {
            type Value = WorkflowCategoryNode;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a category map")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut node = WorkflowCategoryNode::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "workflows" {
                        node.workflows = map.next_value()?;
                    } else {
                        let child = map
                            .next_value()
                            .map_err(|e| serde::de::Error::custom(format!("{key}: {e}")))?;
                        node.children.insert(key, child);
                    }
                }
                Ok(node)
            }
        }
        deserializer.deserialize_map(NodeVisitor)
    }
}
/// Validates category names, workflow names, and descriptions.
///
/// # Errors
/// Returns an error when a name or description is invalid or descriptions conflict.
pub fn validate_workflow_categories(
    categories: &IndexMap<String, WorkflowCategoryNode>,
) -> Result<()> {
    fn visit(
        nodes: &IndexMap<String, WorkflowCategoryNode>,
        parent: &str,
        descriptions: &mut HashMap<String, String>,
    ) -> Result<()> {
        for (name, node) in nodes {
            let path = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent} > {name}")
            };
            if name.trim().is_empty() || name.trim() != name {
                return Err(CruiseError::Other(format!(
                    "invalid category name at {path}"
                )));
            }
            for entry in &node.workflows {
                if entry.name.trim().is_empty() || entry.name.trim() != entry.name {
                    return Err(CruiseError::Other(format!(
                        "invalid workflow name at {path}"
                    )));
                }
                if let Some(desc) = &entry.description {
                    if desc.trim().is_empty() {
                        return Err(CruiseError::Other(format!("blank description at {path}")));
                    }
                    let key = entry
                        .name
                        .strip_prefix("builtin:")
                        .filter(|name| {
                            crate::builtin_workflows::BUILTIN_WORKFLOWS
                                .iter()
                                .any(|workflow| workflow.name == *name)
                        })
                        .unwrap_or(&entry.name);
                    if let Some(old) = descriptions.insert(key.to_string(), desc.clone())
                        && old != *desc
                    {
                        return Err(CruiseError::Other(format!(
                            "conflicting descriptions for {} at {path}",
                            entry.name
                        )));
                    }
                }
            }
            visit(&node.children, &path, descriptions)?;
        }
        Ok(())
    }
    visit(categories, "", &mut HashMap::new())
}
#[derive(Clone, Debug)]
pub struct PickerCandidate {
    pub value: String,
    pub label: String,
    pub key: String,
    pub is_builtin: bool,
    pub is_local: bool,
    pub is_installed_package: bool,
    pub description: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum PickerNode {
    Category {
        name: String,
        nodes: Vec<PickerNode>,
    },
    Workflow {
        candidate: usize,
        description: Option<String>,
    },
}
#[must_use]
pub fn build_picker_tree(
    candidates: &[PickerCandidate],
    categories: &IndexMap<String, WorkflowCategoryNode>,
    show_others: bool,
    others_name: &str,
) -> Vec<PickerNode> {
    fn build(
        categories: &IndexMap<String, WorkflowCategoryNode>,
        candidates: &[PickerCandidate],
        matched: &mut [bool],
    ) -> Vec<PickerNode> {
        let mut result = Vec::new();
        for (name, node) in categories {
            let mut nodes = Vec::new();
            for entry in &node.workflows {
                for (i, candidate) in candidates.iter().enumerate() {
                    let is_match = if let Some(key) = entry.name.strip_prefix("builtin:") {
                        candidate.is_builtin && candidate.key == key
                    } else {
                        candidate.key == entry.name
                    };
                    if is_match {
                        matched[i] = true;
                        nodes.push(PickerNode::Workflow {
                            candidate: i,
                            description: entry
                                .description
                                .clone()
                                .or_else(|| candidate.description.clone()),
                        });
                    }
                }
            }
            nodes.extend(build(&node.children, candidates, matched));
            if !nodes.is_empty() {
                result.push(PickerNode::Category {
                    name: name.clone(),
                    nodes,
                });
            }
        }
        result
    }
    let defaults;
    let categories = if categories.is_empty() {
        defaults = crate::builtin_workflows::DEFAULT_WORKFLOW_CATEGORIES
            .iter()
            .map(|(name, entries)| {
                (
                    (*name).to_string(),
                    WorkflowCategoryNode {
                        workflows: entries
                            .iter()
                            .map(|name| WorkflowEntry {
                                name: (*name).to_string(),
                                description: None,
                            })
                            .collect(),
                        children: IndexMap::new(),
                    },
                )
            })
            .collect();
        &defaults
    } else {
        categories
    };
    let mut matched = vec![false; candidates.len()];
    let declared = build(categories, candidates, &mut matched);
    let mut root = Vec::new();
    let mut packages = Vec::new();
    let mut others = Vec::new();
    for (i, candidate) in candidates.iter().enumerate() {
        if matched[i] {
            continue;
        }
        let leaf = PickerNode::Workflow {
            candidate: i,
            description: candidate.description.clone(),
        };
        if candidate.is_local {
            root.push(leaf);
        } else if candidate.is_installed_package && !candidate.is_builtin {
            packages.push(leaf);
        } else if show_others {
            others.push(leaf);
        }
    }
    root.extend(declared);
    if !packages.is_empty() {
        root.push(PickerNode::Category {
            name: PACKAGES_CATEGORY_NAME.into(),
            nodes: packages,
        });
    }
    if !others.is_empty() {
        root.push(PickerNode::Category {
            name: others_name.into(),
            nodes: others,
        });
    }
    root
}
