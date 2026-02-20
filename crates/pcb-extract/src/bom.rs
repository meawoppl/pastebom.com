use crate::types::*;
use std::collections::HashMap;

/// Configuration for BOM generation.
#[derive(Debug, Clone)]
pub struct BomConfig {
    /// Fields to include in BOM output (e.g. "Value", "Footprint").
    pub fields: Vec<String>,
    /// Attributes to skip (e.g. "virtual", "dnp").
    pub skip_attrs: Vec<String>,
    /// References to skip (e.g. test points).
    pub skip_refs: Vec<String>,
}

impl Default for BomConfig {
    fn default() -> Self {
        Self {
            fields: vec!["Value".to_string(), "Footprint".to_string()],
            skip_attrs: vec!["virtual".to_string()],
            skip_refs: vec![],
        }
    }
}

/// Generate BOM data from footprints and components.
pub fn generate_bom(
    _footprints: &[Footprint],
    components: &[Component],
    config: &BomConfig,
) -> BomData {
    // Build fields map: footprint_index -> [field_values]
    let mut fields_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut skipped: Vec<usize> = Vec::new();

    for comp in components {
        let idx_str = comp.footprint_index.to_string();
        let mut field_vals = Vec::new();
        for field_name in &config.fields {
            let val = match field_name.as_str() {
                "Value" => comp.val.clone(),
                "Footprint" => comp.footprint_name.clone(),
                other => comp.extra_fields.get(other).cloned().unwrap_or_default(),
            };
            field_vals.push(val);
        }
        fields_map.insert(idx_str, field_vals);

        // Check if component should be skipped
        let should_skip = config
            .skip_attrs
            .iter()
            .any(|attr| comp.attr.as_deref() == Some(attr))
            || config.skip_refs.iter().any(|r| comp.ref_.starts_with(r));
        if should_skip {
            skipped.push(comp.footprint_index);
        }
    }

    // Group components by (value, footprint) for BOM rows
    let both = group_components(components, &skipped, None);
    let front = group_components(components, &skipped, Some(Side::Front));
    let back = group_components(components, &skipped, Some(Side::Back));

    BomData {
        both,
        front,
        back,
        skipped,
        fields: BomFields(fields_map),
    }
}

/// Group components into BOM rows.
/// Each row is a Vec<(ref_designator, footprint_index)>.
/// Components are grouped by matching (value, footprint_name).
fn group_components(
    components: &[Component],
    skipped: &[usize],
    side_filter: Option<Side>,
) -> Vec<Vec<(String, usize)>> {
    // Group key: (value, footprint_name)
    let mut groups: Vec<(String, String, Vec<(String, usize)>)> = Vec::new();

    for comp in components {
        if skipped.contains(&comp.footprint_index) {
            continue;
        }
        if let Some(side) = side_filter {
            if comp.layer != side {
                continue;
            }
        }

        let key_val = comp.val.clone();
        let key_fp = comp.footprint_name.clone();

        if let Some(group) = groups
            .iter_mut()
            .find(|(v, f, _)| v == &key_val && f == &key_fp)
        {
            group.2.push((comp.ref_.clone(), comp.footprint_index));
        } else {
            groups.push((
                key_val,
                key_fp,
                vec![(comp.ref_.clone(), comp.footprint_index)],
            ));
        }
    }

    // Sort groups by first reference designator
    groups.sort_by(|a, b| natural_sort_key(&a.2[0].0).cmp(&natural_sort_key(&b.2[0].0)));

    // Sort refs within each group
    groups
        .into_iter()
        .map(|(_, _, mut refs)| {
            refs.sort_by(|a, b| natural_sort_key(&a.0).cmp(&natural_sort_key(&b.0)));
            refs
        })
        .collect()
}

/// Natural sort key: split into (prefix, number) for sorting like R1, R2, R10.
fn natural_sort_key(s: &str) -> (String, u64) {
    let prefix_end = s
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let prefix = s[..prefix_end].to_string();
    let num: u64 = s[prefix_end..].parse().unwrap_or(0);
    (prefix, num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_natural_sort_key() {
        assert!(natural_sort_key("R1") < natural_sort_key("R2"));
        assert!(natural_sort_key("R2") < natural_sort_key("R10"));
        assert!(natural_sort_key("C1") < natural_sort_key("R1"));
    }

    #[test]
    fn test_group_components() {
        let components = vec![
            Component {
                ref_: "R1".to_string(),
                val: "10k".to_string(),
                footprint_name: "0805".to_string(),
                layer: Side::Front,
                footprint_index: 0,
                extra_fields: HashMap::new(),
                attr: None,
            },
            Component {
                ref_: "R2".to_string(),
                val: "10k".to_string(),
                footprint_name: "0805".to_string(),
                layer: Side::Front,
                footprint_index: 1,
                extra_fields: HashMap::new(),
                attr: None,
            },
            Component {
                ref_: "C1".to_string(),
                val: "100nF".to_string(),
                footprint_name: "0603".to_string(),
                layer: Side::Front,
                footprint_index: 2,
                extra_fields: HashMap::new(),
                attr: None,
            },
        ];

        let groups = group_components(&components, &[], None);
        assert_eq!(groups.len(), 2);
        // C1 comes first alphabetically
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].0, "C1");
        // R1, R2 grouped
        assert_eq!(groups[1].len(), 2);
        assert_eq!(groups[1][0].0, "R1");
        assert_eq!(groups[1][1].0, "R2");
    }
}
