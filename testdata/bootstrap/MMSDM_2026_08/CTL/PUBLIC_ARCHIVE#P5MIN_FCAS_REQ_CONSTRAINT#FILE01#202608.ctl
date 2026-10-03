-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#P5MIN_FCAS_REQ_CONSTRAINT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_FCAS_REQ_CONSTRAINT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#P5MIN_FCAS_REQ_CONSTRAINT#FILE01#202608010000.CSV
APPEND INTO TABLE P5MIN_FCAS_REQ_CONSTRAINT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUNNO FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
REGIONID CHAR(20),
BIDTYPE CHAR(10),
LHS FLOAT EXTERNAL,
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
RRP FLOAT EXTERNAL,
REGIONAL_ENABLEMENT FLOAT EXTERNAL,
CONSTRAINT_ENABLEMENT FLOAT EXTERNAL,
REGION_BASE_COST FLOAT EXTERNAL,
BASE_COST FLOAT EXTERNAL,
ADJUSTED_COST FLOAT EXTERNAL,
P_REGULATION FLOAT EXTERNAL)
-- End of Script
