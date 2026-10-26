-- Revision 0007 (predictor fits): the statements Alembic's `0007` upgrade runs, in order: the `predictor_fits` table and its index.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE predictor_fits (
	id VARCHAR NOT NULL, 
	workspace_id VARCHAR NOT NULL, 
	model VARCHAR(200) NOT NULL, 
	created_at VARCHAR(32) NOT NULL, 
	data_cutoff VARCHAR(32), 
	trial_ids TEXT NOT NULL, 
	feature_names TEXT NOT NULL, 
	repositories TEXT NOT NULL, 
	coef TEXT NOT NULL, 
	penalty TEXT NOT NULL, 
	n INTEGER NOT NULL, 
	converged BOOLEAN NOT NULL, 
	metrics TEXT NOT NULL, 
	samples TEXT NOT NULL, 
	CONSTRAINT pk_predictor_fits PRIMARY KEY (id), 
	CONSTRAINT fk_predictor_fits_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT
)
----
CREATE INDEX ix_predictor_fits_workspace_id ON predictor_fits (workspace_id)
