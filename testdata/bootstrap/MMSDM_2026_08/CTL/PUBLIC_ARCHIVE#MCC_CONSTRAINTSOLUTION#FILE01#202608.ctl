-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MCC_CONSTRAINTSOLUTION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MCC_CONSTRAINTSOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MCC_CONSTRAINTSOLUTION#FILE01#202608010000.CSV
APPEND INTO TABLE MCC_CONSTRAINTSOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL)
-- End of Script
