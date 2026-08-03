//! Partitioning resolved predicates into regex and decoded residual filters.

use crate::expression::{Predicate, Program, ProgramNode};
use crate::{Error, Result};

pub(super) struct PredicatePartition<'statement> {
    pub(super) regex_by_source: Vec<Vec<Predicate<'statement>>>,
    pub(super) local_residuals: Vec<Option<Program<'statement>>>,
    pub(super) cross_source_residual: Option<Program<'statement>>,
}

pub(super) fn partition<'statement>(
    program: Option<Program<'statement>>,
    source_count: usize,
) -> Result<PredicatePartition<'statement>> {
    let mut regex_by_source = Vec::new();
    regex_by_source
        .try_reserve_exact(source_count)
        .map_err(|_| Error::Allocation {
            operation: "reserving query predicate buckets",
        })?;
    regex_by_source.resize_with(source_count, Vec::new);

    let mut local_builders = Vec::new();
    local_builders
        .try_reserve_exact(source_count)
        .map_err(|_| Error::Allocation {
            operation: "reserving source-local residual builders",
        })?;
    local_builders.resize_with(source_count, ResidualBuilder::default);
    let mut cross_source_builder = ResidualBuilder::default();

    if let Some(program) = program {
        partition_program(
            program,
            &mut regex_by_source,
            &mut local_builders,
            &mut cross_source_builder,
        )?;
    }

    let mut local_residuals = Vec::new();
    local_residuals
        .try_reserve_exact(source_count)
        .map_err(|_| Error::Allocation {
            operation: "reserving source-local residual programs",
        })?;
    for builder in local_builders {
        local_residuals.push(builder.finish()?);
    }

    Ok(PredicatePartition {
        regex_by_source,
        local_residuals,
        cross_source_residual: cross_source_builder.finish()?,
    })
}

fn partition_program<'statement>(
    program: Program<'statement>,
    regex_by_source: &mut [Vec<Predicate<'statement>>],
    local_builders: &mut [ResidualBuilder<'statement>],
    cross_source_builder: &mut ResidualBuilder<'statement>,
) -> Result<()> {
    let root_children = match program.nodes().first() {
        Some(ProgramNode::And { children }) => Some(*children),
        Some(ProgramNode::Or { .. } | ProgramNode::Predicate(_)) => None,
        None => {
            return Err(Error::Capacity {
                operation: "partitioning an empty WHERE expression",
            });
        }
    };

    let Some(root_children) = root_children else {
        let destination = classify_factor(program.nodes(), regex_by_source.len())?;
        return keep_whole_factor(
            destination,
            program,
            regex_by_source,
            local_builders,
            cross_source_builder,
        );
    };

    let mut nodes = program.into_nodes().into_iter();
    if !matches!(nodes.next(), Some(ProgramNode::And { children }) if children == root_children) {
        return Err(Error::Capacity {
            operation: "reading the top-level WHERE conjunction",
        });
    }

    for _ in 0..root_children {
        let factor_len = subtree_len(nodes.as_slice())?;
        let factor = nodes.as_slice().get(..factor_len).ok_or(Error::Capacity {
            operation: "reading a top-level WHERE factor",
        })?;
        let destination = classify_factor(factor, regex_by_source.len())?;
        move_factor(
            destination,
            factor_len,
            &mut nodes,
            regex_by_source,
            local_builders,
            cross_source_builder,
        )?;
    }

    if !nodes.as_slice().is_empty() {
        return Err(Error::Capacity {
            operation: "finishing top-level WHERE partitioning",
        });
    }
    Ok(())
}

fn keep_whole_factor<'statement>(
    destination: FactorDestination,
    program: Program<'statement>,
    regex_by_source: &mut [Vec<Predicate<'statement>>],
    local_builders: &mut [ResidualBuilder<'statement>],
    cross_source_builder: &mut ResidualBuilder<'statement>,
) -> Result<()> {
    match destination {
        FactorDestination::Regex(source) => {
            let factor_len = program.nodes().len();
            let mut nodes = program.into_nodes().into_iter();
            move_factor(
                FactorDestination::Regex(source),
                factor_len,
                &mut nodes,
                regex_by_source,
                local_builders,
                cross_source_builder,
            )?;
            if !nodes.as_slice().is_empty() {
                return Err(Error::Capacity {
                    operation: "finishing WHERE root partitioning",
                });
            }
            Ok(())
        }
        FactorDestination::Local(source) => local_builders
            .get_mut(source)
            .ok_or(Error::Capacity {
                operation: "selecting a source-local residual builder",
            })?
            .use_program(program),
        FactorDestination::CrossSource => cross_source_builder.use_program(program),
    }
}

#[derive(Clone, Copy)]
enum FactorDestination {
    Regex(usize),
    Local(usize),
    CrossSource,
}

fn classify_factor(factor: &[ProgramNode<'_>], source_count: usize) -> Result<FactorDestination> {
    if let [ProgramNode::Predicate(predicate)] = factor
        && is_safe_regex_predicate(predicate)
    {
        let source = predicate.column().source;
        validate_source(source, source_count)?;
        return Ok(FactorDestination::Regex(source));
    }

    match factor_sources(factor, source_count)? {
        FactorSources::Local(source) => Ok(FactorDestination::Local(source)),
        FactorSources::CrossSource => Ok(FactorDestination::CrossSource),
    }
}

const fn is_safe_regex_predicate(predicate: &Predicate<'_>) -> bool {
    matches!(
        predicate,
        Predicate::Equal { .. }
            | Predicate::NotEqual { .. }
            | Predicate::Like { .. }
            | Predicate::IsNull { .. }
            | Predicate::IsNotNull { .. }
    )
}

fn move_factor<'statement>(
    destination: FactorDestination,
    factor_len: usize,
    nodes: &mut std::vec::IntoIter<ProgramNode<'statement>>,
    regex_by_source: &mut [Vec<Predicate<'statement>>],
    local_builders: &mut [ResidualBuilder<'statement>],
    cross_source_builder: &mut ResidualBuilder<'statement>,
) -> Result<()> {
    match destination {
        FactorDestination::Regex(source) => {
            if factor_len != 1 {
                return Err(Error::Capacity {
                    operation: "moving a non-leaf regex predicate",
                });
            }
            let bucket = regex_by_source.get_mut(source).ok_or(Error::Capacity {
                operation: "selecting a query predicate bucket",
            })?;
            bucket.try_reserve(1).map_err(|_| Error::Allocation {
                operation: "growing a query predicate bucket",
            })?;
            let Some(ProgramNode::Predicate(predicate)) = nodes.next() else {
                return Err(Error::Capacity {
                    operation: "moving a regex predicate",
                });
            };
            bucket.push(predicate);
            Ok(())
        }
        FactorDestination::Local(source) => local_builders
            .get_mut(source)
            .ok_or(Error::Capacity {
                operation: "selecting a source-local residual builder",
            })?
            .push_factor(nodes, factor_len),
        FactorDestination::CrossSource => cross_source_builder.push_factor(nodes, factor_len),
    }
}

enum FactorSources {
    Local(usize),
    CrossSource,
}

fn factor_sources(factor: &[ProgramNode<'_>], source_count: usize) -> Result<FactorSources> {
    let mut first_source = None;
    let mut crosses_sources = false;
    for node in factor {
        let ProgramNode::Predicate(predicate) = node else {
            continue;
        };
        let source = predicate.column().source;
        validate_source(source, source_count)?;
        match first_source {
            Some(first) if first != source => crosses_sources = true,
            Some(_) => {}
            None => first_source = Some(source),
        }
    }

    let first_source = first_source.ok_or(Error::Capacity {
        operation: "classifying a WHERE factor without predicates",
    })?;
    if crosses_sources {
        Ok(FactorSources::CrossSource)
    } else {
        Ok(FactorSources::Local(first_source))
    }
}

fn validate_source(source: usize, source_count: usize) -> Result<()> {
    if source < source_count {
        Ok(())
    } else {
        Err(Error::Schema(format!(
            "resolved predicate source {source} is outside the query sources"
        )))
    }
}

fn subtree_len(nodes: &[ProgramNode<'_>]) -> Result<usize> {
    let mut pending = 1_usize;
    for (index, node) in nodes.iter().enumerate() {
        pending = pending.checked_sub(1).ok_or(Error::Capacity {
            operation: "counting a top-level WHERE factor",
        })?;
        pending = pending
            .checked_add(node.child_count())
            .ok_or(Error::Capacity {
                operation: "counting top-level WHERE descendants",
            })?;
        if pending == 0 {
            return index.checked_add(1).ok_or(Error::Capacity {
                operation: "sizing a top-level WHERE factor",
            });
        }
    }
    Err(Error::Capacity {
        operation: "reading a complete top-level WHERE factor",
    })
}

#[derive(Default)]
struct ResidualBuilder<'statement> {
    factors: usize,
    nodes: Vec<ProgramNode<'statement>>,
}

impl<'statement> ResidualBuilder<'statement> {
    fn use_program(&mut self, program: Program<'statement>) -> Result<()> {
        if self.factors != 0 || !self.nodes.is_empty() {
            return Err(Error::Capacity {
                operation: "retaining a complete residual expression program",
            });
        }
        self.factors = 1;
        self.nodes = program.into_nodes();
        Ok(())
    }

    fn push_factor(
        &mut self,
        nodes: &mut std::vec::IntoIter<ProgramNode<'statement>>,
        factor_len: usize,
    ) -> Result<()> {
        let next_factors = self.factors.checked_add(1).ok_or(Error::Capacity {
            operation: "counting residual WHERE factors",
        })?;
        let root_slot = usize::from(self.factors == 1);
        let additional = factor_len.checked_add(root_slot).ok_or(Error::Capacity {
            operation: "sizing a residual expression program",
        })?;
        self.nodes
            .try_reserve(additional)
            .map_err(|_| Error::Allocation {
                operation: "growing a residual expression program",
            })?;
        if self.factors == 1 {
            self.nodes.insert(0, ProgramNode::And { children: 0 });
        }
        for _ in 0..factor_len {
            self.nodes.push(nodes.next().ok_or(Error::Capacity {
                operation: "moving a residual WHERE factor",
            })?);
        }
        self.factors = next_factors;
        Ok(())
    }

    fn finish(mut self) -> Result<Option<Program<'statement>>> {
        match self.factors {
            0 => Ok(None),
            1 => Ok(Some(Program::new(self.nodes))),
            children => {
                let Some(root) = self.nodes.first_mut() else {
                    return Err(Error::Capacity {
                        operation: "finishing a residual expression program",
                    });
                };
                *root = ProgramNode::And { children };
                Ok(Some(Program::new(self.nodes)))
            }
        }
    }
}

#[cfg(test)]
mod tests;
