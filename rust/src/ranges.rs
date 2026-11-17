use std::ops::Range;

use crate::fs::byte_range;
use crate::{ByteRange, Condition, FsError, FsResult, RangeReadOptions};

pub(crate) fn validate_options(options: &RangeReadOptions) -> FsResult<()> {
    if options.max_request_size == 0 || options.max_concurrency == 0 {
        return Err(FsError::InvalidArgument(
            "max_request_size and max_concurrency must be positive".into(),
        ));
    }
    if matches!(options.condition, Some(Condition::Missing)) {
        return Err(FsError::InvalidArgument(
            "the Missing condition is only valid for writes".into(),
        ));
    }
    Ok(())
}

pub(crate) struct RangePlan {
    ranges: Vec<Range<u64>>,
    pub requests: Vec<Range<u64>>,
}

impl RangePlan {
    /// `options` must pass `validate_options` before constructing a plan.
    pub fn new(size: u64, ranges: &[ByteRange], options: &RangeReadOptions) -> Self {
        let ranges: Vec<_> = ranges
            .iter()
            .map(|range| {
                let (start, end) = byte_range(size, range.start, range.end);
                start..end
            })
            .collect();
        let mut sorted: Vec<_> = ranges
            .iter()
            .filter(|range| !range.is_empty())
            .cloned()
            .collect();
        sorted.sort_unstable_by_key(|range| (range.start, range.end));
        let mut covered: Vec<Range<u64>> = Vec::new();
        for range in sorted {
            if let Some(last) = covered.last_mut() {
                if range.start <= last.end {
                    last.end = last.end.max(range.end);
                    continue;
                }
            }
            covered.push(range);
        }
        let mut requests: Vec<Range<u64>> = Vec::new();
        for range in covered {
            let mut start = range.start;
            if let Some(last) = requests.last_mut() {
                let limit = last.start.saturating_add(options.max_request_size);
                if start <= last.end.saturating_add(options.max_gap) && start < limit {
                    last.end = range.end.min(limit);
                    start = last.end;
                }
            }
            while start < range.end {
                let end = start
                    .saturating_add(options.max_request_size)
                    .min(range.end);
                requests.push(start..end);
                start = end;
            }
        }
        Self { ranges, requests }
    }

    pub fn output(&self) -> FsResult<Vec<Vec<u8>>> {
        self.ranges
            .iter()
            .map(|range| {
                let length = usize::try_from(range.end - range.start).map_err(|_| {
                    FsError::InvalidArgument("range exceeds addressable memory".into())
                })?;
                Ok(vec![0; length])
            })
            .collect()
    }

    pub fn copy_response(
        &self,
        request: &Range<u64>,
        bytes: &[u8],
        output: &mut [Vec<u8>],
    ) -> FsResult<()> {
        if bytes.len() as u64 != request.end - request.start {
            return Err(FsError::Other(format!(
                "range response length mismatch: expected {}, received {}",
                request.end - request.start,
                bytes.len(),
            )));
        }
        for (range, target) in self.ranges.iter().zip(output) {
            let start = range.start.max(request.start);
            let end = range.end.min(request.end);
            if start < end {
                let source_start = (start - request.start) as usize;
                let target_start = (start - range.start) as usize;
                let length = (end - start) as usize;
                target[target_start..target_start + length]
                    .copy_from_slice(&bytes[source_start..source_start + length]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: i64, end: i64) -> ByteRange {
        ByteRange {
            start: Some(start),
            end: Some(end),
        }
    }

    #[test]
    fn planner_coalesces_splits_and_restores_input_order() {
        let ranges = [
            range(20, 24),
            range(0, 4),
            range(2, 8),
            range(10, 12),
            range(2, 8),
            range(7, 3),
            range(-4, 40),
        ];
        let options = RangeReadOptions {
            max_gap: 2,
            max_request_size: 6,
            ..Default::default()
        };
        let plan = RangePlan::new(24, &ranges, &options);
        assert_eq!(plan.requests, vec![0..6, 6..12, 20..24]);
        let source: Vec<_> = (0..24).collect();
        let mut output = plan.output().unwrap();
        for request in plan.requests.iter().rev() {
            plan.copy_response(
                request,
                &source[request.start as usize..request.end as usize],
                &mut output,
            )
            .unwrap();
        }
        assert_eq!(
            output,
            vec![
                source[20..24].to_vec(),
                source[0..4].to_vec(),
                source[2..8].to_vec(),
                source[10..12].to_vec(),
                source[2..8].to_vec(),
                vec![],
                source[20..24].to_vec()
            ]
        );
    }

    #[test]
    fn planner_does_not_fetch_requests_containing_only_gaps() {
        let plan = RangePlan::new(
            100,
            &[range(0, 1), range(40, 41)],
            &RangeReadOptions {
                max_gap: 100,
                max_request_size: 4,
                ..Default::default()
            },
        );
        assert_eq!(plan.requests, vec![0..1, 40..41]);
        let plan = RangePlan::new(
            20,
            &[range(0, 2), range(3, 10)],
            &RangeReadOptions {
                max_gap: 1,
                max_request_size: 4,
                ..Default::default()
            },
        );
        assert_eq!(plan.requests, vec![0..4, 4..8, 8..10]);
    }

    #[test]
    fn normalization_handles_extreme_signed_offsets_and_large_sizes() {
        assert_eq!(byte_range(10, Some(i64::MIN), Some(i64::MAX)), (0, 10));
        assert_eq!(
            byte_range(u64::MAX, Some(-3), None),
            (u64::MAX - 3, u64::MAX)
        );
        assert_eq!(byte_range(10, Some(20), Some(-20)), (10, 10));
    }

    #[test]
    fn planner_matches_individual_ranges_across_gap_and_size_limits() {
        let source: Vec<_> = (0..32).collect();
        let ranges: Vec<_> = (-40..40)
            .step_by(3)
            .flat_map(|start| (-40..40).step_by(5).map(move |end| range(start, end)))
            .collect();
        for max_gap in [0, 1, 4, u64::MAX] {
            for max_request_size in [1, 3, 8, u64::MAX] {
                let options = RangeReadOptions {
                    max_gap,
                    max_request_size,
                    ..Default::default()
                };
                let plan = RangePlan::new(source.len() as u64, &ranges, &options);
                let mut output = plan.output().unwrap();
                for request in plan.requests.iter().rev() {
                    assert!(request.end - request.start <= max_request_size);
                    plan.copy_response(
                        request,
                        &source[request.start as usize..request.end as usize],
                        &mut output,
                    )
                    .unwrap();
                }
                for (range, actual) in ranges.iter().zip(output) {
                    let (start, end) = byte_range(source.len() as u64, range.start, range.end);
                    assert_eq!(actual, source[start as usize..end as usize]);
                }
            }
        }
    }

    #[test]
    fn planner_rejects_incorrect_response_lengths() {
        let plan = RangePlan::new(8, &[range(0, 4)], &RangeReadOptions::default());
        for bytes in [b"abc".as_slice(), b"abcde".as_slice()] {
            assert!(matches!(
                plan.copy_response(&plan.requests[0], bytes, &mut plan.output().unwrap()),
                Err(FsError::Other(_))
            ));
        }
    }
}
