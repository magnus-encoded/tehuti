pub fn git_metadata(mut output: impl FnMut(&[&str]) -> Option<String>) -> (String, Vec<String>) {
    let metadata = (|| {
        let hash = output(&["rev-parse", "--short=9", "HEAD"])?;
        let mut paths = Vec::new();
        for reference in ["HEAD", "packed-refs"] {
            paths.push(output(&["rev-parse", "--git-path", reference])?);
        }
        let reference = output(&["rev-parse", "--symbolic-full-name", "HEAD"])?;
        if reference != "HEAD" {
            paths.push(output(&["rev-parse", "--git-path", &reference])?);
        }
        Some((hash, paths))
    })();
    metadata.unwrap_or_else(|| ("unknown".to_owned(), Vec::new()))
}

#[must_use]
pub fn build_date(epoch: Option<&str>) -> String {
    epoch
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .filter(|date| (0..=9999).contains(&chrono::Datelike::year(date)))
        .map_or_else(
            || "unknown".to_owned(),
            |date| date.format("%Y-%m-%d").to_string(),
        )
}
