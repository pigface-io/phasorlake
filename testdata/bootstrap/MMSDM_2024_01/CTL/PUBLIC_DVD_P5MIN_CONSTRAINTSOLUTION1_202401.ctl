-- ************************************************************
-- Title:	PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION1_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_CONSTRAINTSOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION1_202401010000.CSV
APPEND INTO TABLE P5MIN_CONSTRAINTSOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
INTERVENTION FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
CONSTRAINTID CHAR(20),
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
VIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
CONFIDENTIAL_TO FILLER,
GENCONID_EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
GENCONID_VERSIONNO FLOAT EXTERNAL,
LHS FLOAT EXTERNAL)
-- End of Script
