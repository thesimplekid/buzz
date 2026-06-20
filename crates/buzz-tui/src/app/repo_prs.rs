use super::{clamp_index, short_id, App, Focus};
use buzz_sdk::GitStatus;

impl App {
    pub async fn focus_repo_pull_requests(&mut self) {
        if self.repos.get(self.selected_repo).is_none() {
            self.status = "Select a repo before opening pull requests".to_string();
            return;
        }
        self.focus = Focus::RepoPullRequests;
        self.refresh_repo_pull_requests().await;
    }

    pub async fn refresh_repo_pull_requests(&mut self) {
        let Some(repo) = self.repos.get(self.selected_repo).cloned() else {
            self.repo_pull_requests.clear();
            self.selected_repo_pull_request = 0;
            return;
        };
        match self
            .list_repo_pull_requests_native(&repo.owner, &repo.dtag)
            .await
        {
            Ok(pull_requests) => {
                self.repo_pull_requests = pull_requests;
                clamp_index(
                    &mut self.selected_repo_pull_request,
                    self.repo_pull_requests.len(),
                );
                self.status = format!(
                    "Loaded {} pull request{}",
                    self.repo_pull_requests.len(),
                    if self.repo_pull_requests.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            Err(error) => self.status = format!("pull requests: {error}"),
        }
    }

    pub async fn review_selected_pull_request(&mut self, approve: bool) {
        let Some(pull_request) = self
            .repo_pull_requests
            .get(self.selected_repo_pull_request)
            .cloned()
        else {
            self.status = "No pull request selected".to_string();
            return;
        };
        match self
            .review_pull_request_native(&pull_request, approve)
            .await
        {
            Ok(()) => {
                self.refresh_repo_pull_requests().await;
                self.status = format!(
                    "{} pull request {}",
                    if approve {
                        "Approved"
                    } else {
                        "Requested changes on"
                    },
                    short_id(&pull_request.id)
                );
            }
            Err(error) => self.status = format!("pull request review: {error}"),
        }
    }

    pub async fn set_selected_pull_request_status(&mut self, status: GitStatus) {
        let Some(pull_request) = self
            .repo_pull_requests
            .get(self.selected_repo_pull_request)
            .cloned()
        else {
            self.status = "No pull request selected".to_string();
            return;
        };
        match self
            .set_pull_request_status_native(&pull_request, status)
            .await
        {
            Ok(()) => {
                self.refresh_repo_pull_requests().await;
                self.status = format!(
                    "Updated pull request {} to {}",
                    short_id(&pull_request.id),
                    match status {
                        GitStatus::Open => "open",
                        GitStatus::Draft => "draft",
                        GitStatus::Closed => "closed",
                        GitStatus::AppliedOrResolved => "merged",
                    }
                );
            }
            Err(error) => self.status = format!("pull request status: {error}"),
        }
    }
}
