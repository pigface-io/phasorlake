-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#RESIDUE_CON_FUNDS#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_CON_FUNDS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#RESIDUE_CON_FUNDS#FILE01#202608010000.CSV
APPEND INTO TABLE RESIDUE_CON_FUNDS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTID CHAR(30),
INTERCONNECTORID CHAR(10),
FROMREGIONID CHAR(10),
DEFAULTUNITS FLOAT EXTERNAL,
ROLLOVERUNITS FLOAT EXTERNAL,
REALLOCATEDUNITS FLOAT EXTERNAL,
UNITSOFFERED FLOAT EXTERNAL,
MEANRESERVEPRICE FLOAT EXTERNAL,
SCALEFACTOR FLOAT EXTERNAL,
ACTUALRESERVEPRICE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
