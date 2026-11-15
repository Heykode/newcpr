//! Global published analytics export, including restored history.

pub(crate) fn backup_usage_export_sql() -> String {
    "COPY (
         WITH counters AS (
           SELECT bucket_start,c.key,sum(c.value::numeric) as value
           FROM request_analytics_1h CROSS JOIN LATERAL jsonb_each_text(counts) c
           WHERE collection='global' AND outcome <> 'running'
             AND c.key IN ('request_count','success_count','failure_count','cancelled_count','incomplete_count',
               'caller_error_count','input_tokens','output_tokens','cached_tokens','cache_write_tokens','reasoning_tokens','total_tokens')
           GROUP BY bucket_start,c.key
         ), totals_native AS (
           SELECT bucket_start,jsonb_object_agg(key,value) as counts FROM counters GROUP BY bucket_start
         ), amounts AS (
           SELECT bucket_start,c.key as cost_currency,sum(c.value::numeric)::text as amount
           FROM request_analytics_1h CROSS JOIN LATERAL jsonb_each_text(costs) c
           WHERE collection='global' AND outcome <> 'running' AND c.key NOT IN ('standard_usd','no_cache_usd')
           GROUP BY bucket_start,c.key
         ), live_costs AS (
           SELECT bucket_start,jsonb_object_agg(cost_currency,amount) AS costs FROM amounts GROUP BY bucket_start
         ), hours AS (
           SELECT bucket_start,counts,coalesce(costs,'{}'::jsonb) AS costs
           FROM totals_native LEFT JOIN live_costs USING(bucket_start)
           UNION ALL SELECT bucket_start,counts,costs FROM backup_usage_hours
         ), combined_counts AS (
           SELECT bucket_start,c.key,sum(c.value::numeric) AS value
           FROM hours CROSS JOIN LATERAL jsonb_each_text(counts) c GROUP BY bucket_start,c.key
         ), combined_costs AS (
           SELECT bucket_start,c.key,sum(c.value::numeric)::text AS value
           FROM hours CROSS JOIN LATERAL jsonb_each_text(costs) c GROUP BY bucket_start,c.key
         ), totals AS (
           SELECT bucket_start,jsonb_object_agg(key,value) AS counts
           FROM combined_counts GROUP BY bucket_start
         ), costs AS (
           SELECT bucket_start,jsonb_object_agg(key,value) AS costs
           FROM combined_costs GROUP BY bucket_start
         )
         SELECT bucket_start,counts,coalesce(costs,'{}'::jsonb)
         FROM totals LEFT JOIN costs USING(bucket_start) ORDER BY bucket_start
         ) TO STDOUT".to_owned()
}
